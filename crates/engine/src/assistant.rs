//! Local, data-driven editing assistance. Analysis only proposes cuts; explicit Apply uses the
//! ordinary edit algebra in one transaction. External planners can submit the same bounded schema.
use std::sync::{Arc, Mutex};

use filmcraft_media::{MediaSource, SharedSource};
use filmcraft_project::{ItemId, Project};
use filmcraft_render::SourceProvider;
use filmcraft_time::{Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, has_seq};
use crate::{EngineError, Result, Session};

const MAX_SECONDS: f64 = 600.0;
const MAX_ACTIONS: usize = 256;

#[derive(Default)]
pub struct Assistant {
    next_token: u64,
    plan: Option<Plan>,
    work: Option<Analysis>,
    error: Option<String>,
}

impl Assistant {
    /// Hosts must keep polling even when the Assistant tab is hidden.
    pub fn is_analysing(&self) -> bool {
        self.work.is_some()
    }
}

struct Plan {
    token: u64,
    project: Arc<Project>,
    sequence: ItemId,
    ranges: Vec<TimeRange>,
    matches: Vec<TimeRange>,
    seek: Option<Tick>,
    label: String,
    actions: Vec<Action>,
    result: Option<Arc<Project>>,
}

struct Analysis {
    project: Arc<Project>,
    sequence: ItemId,
    provider: CheckedProvider,
    sample: i64,
    total: i64,
    silence_start: Option<i64>,
    ranges: Vec<TimeRange>,
    threshold: f64,
    min: Tick,
    keep: Tick,
    target: Option<Tick>,
    started: web_time::Instant,
}

/// Restricted proposal vocabulary: no arbitrary commands, paths, scripts or project writes.
#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Action {
    #[serde(rename = "extract")]
    Extract { start: f64, end: f64 },
    #[serde(rename = "seek")]
    Seek { time: f64 },
    #[serde(rename = "split")]
    Split { clip: u64, time: f64 },
    #[serde(rename = "move")]
    Move { clip: u64, start: f64, track: u64 },
    #[serde(rename = "trim")]
    Trim { clip: u64, edge: String, delta: f64 },
    #[serde(rename = "gain")]
    Gain { clip: u64, db: f64 },
}

fn spec(id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool) -> CommandSpec {
    CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: if id == "assistant.inspect" || id == "assistant.cancel" { always } else { has_seq },
        run,
        journal,
    }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "assistant.plan",
            "Preview Assistant Edit",
            r#"{"instruction":str?,"actions":[{type:extract,start:seconds,end:seconds}|{type:seek,time:seconds}|{type:split,clip:id,time:seconds}|{type:move,clip:id,start:seconds,track:id}|{type:trim,clip:id,edge:in|out,delta:seconds}|{type:gain,clip:id,db:number}]?,"thresholdDb":-96..-6=-40,"minSeconds":0.1..30=0.4,"keepSeconds":0..1=0.1}"#,
            plan,
            false,
        ),
        spec("assistant.inspect", "Inspect Assistant", "{}", inspect, false),
        spec("assistant.context", "Assistant Timeline Context", "{}", context, false),
        spec("assistant.apply", "Apply Reviewed Assistant Edit", r#"{"token":id}"#, apply, true),
        spec("assistant.cancel", "Discard Assistant Proposal", "{}", cancel, false),
    ]
}

fn error(message: impl Into<String>) -> EngineError {
    bad("assistant.plan", message)
}

fn seconds(value: f64) -> Result<Tick> {
    if !value.is_finite() || !(0.0..=MAX_SECONDS).contains(&value) {
        return Err(error("time must be finite and between 0 and 600 seconds"));
    }
    Ok(Tick::from_seconds_f64(value))
}

fn editable(s: &Session, ranges: &[TimeRange]) -> Result<()> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if ranges.len() > MAX_ACTIONS {
        return Err(error("at most 256 cuts per reviewed plan"));
    }
    if !ranges.is_empty()
        && (q.all_tracks().any(|t| t.locked && !t.items.is_empty()) || q.caption_tracks.iter().any(|t| (t.locked || !t.sync_lock) && !t.captions.is_empty()))
    {
        return Err(error("unlock populated tracks and enable caption sync lock before an all-track ripple edit"));
    }
    for r in ranges {
        if r.start < Tick::ZERO || r.duration <= Tick::ZERO || r.start.0.checked_add(r.duration.0).is_none_or(|e| e > q.duration().0) {
            return Err(error("cut range is outside the sequence"));
        }
    }
    Ok(())
}

fn store(s: &mut Session, ranges: Vec<TimeRange>, seek: Option<Tick>, label: String) -> Result<Value> {
    let ranges = filmcraft_edit::transcript::merge_ranges(ranges);
    editable(s, &ranges)?;
    let token = s.assistant.next_token.checked_add(1).ok_or_else(|| error("too many assistant proposals"))?;
    s.assistant.next_token = token;
    s.assistant.work = None;
    s.assistant.error = None;
    s.assistant.plan = Some(Plan {
        token,
        project: s.project.clone(),
        sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        ranges,
        matches: Vec::new(),
        seek,
        label,
        actions: Vec::new(),
        result: None,
    });
    inspect(s, &Value::Null)
}

fn number(p: &Value, key: &str, default: f64, low: f64, high: f64) -> Result<f64> {
    let n = match p.get(key) {
        Some(v) => v.as_f64().ok_or_else(|| error(format!("{key} must be a number")))?,
        None => default,
    };
    if !n.is_finite() || !(low..=high).contains(&n) {
        return Err(error(format!("{key} must be between {low} and {high}")));
    }
    Ok(n)
}

fn checked_references(s: &Session) -> Result<()> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    if rate.num <= 0 || rate.den <= 0 || !(0.1..=240.0).contains(&rate.as_f64()) || !(8000..=192000).contains(&q.settings.sample_rate) {
        return Err(error("invalid sequence rate for AI planning"));
    }
    let mut items = 0usize;
    let mut words = 0usize;
    for item in q.all_tracks().flat_map(|t| &t.items) {
        items = items.saturating_add(1);
        if items > 10000
            || item.start < Tick::ZERO
            || item.duration <= Tick::ZERO
            || item.start.0.checked_add(item.duration.0).is_none_or(|end| end > Tick::MAX.0)
            || !item.speed.is_finite()
            || !(1e-6..=100.0).contains(&item.speed.abs())
        {
            return Err(error("timeline has invalid or excessive clip data for AI planning"));
        }
        if let Some(tr) = s.project.transcripts.get(&item.item) {
            words = words.saturating_add(tr.words.len());
            if words > 10000
                || tr.words.iter().any(|w| w.text.len() > 1024 || w.start < Tick::ZERO || w.end < w.start || w.end > Tick::MAX)
                || tr.speakers.iter().any(|sp| sp.name.len() > 256)
            {
                return Err(error("transcript exceeds AI context bounds; use a shorter reviewed sequence"));
            }
        }
    }
    for item in q.all_tracks().flat_map(|t| &t.items) {
        let mut id = item.item;
        let mut valid = false;
        for _ in 0..16 {
            match s.project.item(id).map(|i| &i.kind) {
                Some(filmcraft_project::ItemKind::Subclip { parent, .. }) => id = *parent,
                Some(_) => {
                    valid = true;
                    break;
                }
                None => break,
            }
        }
        if !valid {
            return Err(error("timeline has missing, cyclic or excessively nested source references"));
        }
    }
    Ok(())
}

fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    // A failed replacement proposal cannot leave an old Apply button armed.
    s.assistant.plan = None;
    s.assistant.work = None;
    s.assistant.error = None;
    checked_references(s)?;
    let object = p.as_object().ok_or_else(|| error("proposal must be an object"))?;
    if object.keys().any(|k| !["instruction", "actions", "schemaVersion", "thresholdDb", "minSeconds", "keepSeconds"].contains(&k.as_str()))
        || p.get("schemaVersion").is_some_and(|v| v.as_u64() != Some(1))
    {
        return Err(error("unsupported proposal schema or unknown fields"));
    }
    if p.get("actions").is_some() && p.get("instruction").is_some() {
        return Err(error("supply instructions or structured actions, not both"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if q.duration() <= Tick::ZERO || q.duration() > seconds(MAX_SECONDS)? {
        return Err(error("open a nonempty sequence of at most ten minutes"));
    }
    if let Some(raw) = p.get("actions") {
        let a = raw.as_array().ok_or_else(|| error("actions must be an array"))?;
        if a.is_empty() || a.len() > MAX_ACTIONS {
            return Err(error("supply 1 to 256 structured actions"));
        }
        let actions: Vec<Action> = serde_json::from_value(raw.clone()).map_err(|e| error(e.to_string()))?;
        let mut ranges = Vec::new();
        let mut seek = None;
        let mut edits = Vec::new();
        for a in actions {
            match a {
                Action::Extract { start, end } => {
                    let (a, b) = (seconds(start)?, seconds(end)?);
                    if b <= a || b > q.duration() {
                        return Err(error("extract needs start < end within the sequence"));
                    }
                    // Inward snapping never deletes a frame outside a proposed cut.
                    let rate = q.frame_rate();
                    let fd = rate.frame_duration();
                    let a2 = rate.snap(a);
                    let a2 = if a2 < a { a2 + fd } else { a2 };
                    let b2 = rate.snap(b);
                    let b2 = if b2 > b { b2 - fd } else { b2 };
                    if b2 <= a2 {
                        return Err(error("cut is shorter than one frame"));
                    }
                    ranges.push(TimeRange::from_bounds(a2, b2));
                }
                Action::Seek { time } => {
                    let t = seconds(time)?;
                    if t >= q.duration() || seek.replace(t).is_some() {
                        return Err(error("supply one seek inside the sequence"));
                    }
                }
                edit => edits.push(edit),
            }
        }
        if !edits.is_empty() && (!ranges.is_empty() || seek.is_some()) {
            return Err(error("clip edits must be separate from ripple extracts and navigation"));
        }
        if !edits.is_empty() {
            let result = stage_edits(s, &edits)?;
            store(s, Vec::new(), None, "Reviewed clip edits · one undo step".into())?;
            if let Some(p) = s.assistant.plan.as_mut() {
                p.actions = edits;
                p.result = Some(result);
            }
            return inspect(s, &Value::Null);
        }
        if seek.is_some() && !ranges.is_empty() {
            return Err(error("a proposal must contain cuts or navigation, not both"));
        }
        return store(s, ranges, seek, "External planner proposal (review every range)".into());
    }
    let instruction = p.get("instruction").and_then(Value::as_str).ok_or_else(|| error("instruction is required"))?;
    if instruction.len() > 4096 {
        return Err(error("instruction is too long"));
    }
    let n = instruction.trim().to_lowercase().replace(['á', 'à'], "a").replace('é', "e").replace('í', "i").replace('ó', "o").replace('ú', "u");
    for prefix in ["busca donde digo ", "busca ", "buscame ", "find ", "search "] {
        if n.strip_prefix(prefix).is_some() {
            let raw = instruction.trim().chars().skip(prefix.chars().count()).collect::<String>();
            let query = raw.trim_matches(['\"', '\'']).trim();
            if query.is_empty() {
                return Err(error("supply text to find"));
            }
            let words = crate::transcript::sequence_words(s);
            let hits = filmcraft_edit::transcript::search(&words, query);
            let mut ranges = Vec::new();
            for hit in &hits {
                if let Some(r) = filmcraft_edit::transcript::word_range(&words, hit.start, hit.end - 1, s.sequence_rate()) {
                    ranges.push(r);
                }
            }
            let seek = ranges.first().map(|r| r.start);
            store(s, Vec::new(), seek, format!("{} transcript matches for {query:?}", ranges.len()))?;
            if let Some(plan) = s.assistant.plan.as_mut() {
                plan.matches = ranges.clone();
            }
            let result = inspect(s, &Value::Null)?;
            return Ok(json!({"state": result, "matches": ranges.iter().map(|r| json!({"start":r.start.0,"end":r.end().0})).collect::<Vec<_>>()}));
        }
    }
    let duration_text = ["version de ", "generame una version de ", "genera una version de ", "haceme una version de ", "shorten to ", "auto edit "]
        .iter()
        .find_map(|prefix| n.strip_prefix(prefix));
    let target = if let Some(text) = duration_text {
        let tokens = text.split_whitespace().collect::<Vec<_>>();
        if tokens.len() > 2 || tokens.get(1).is_some_and(|unit| !["segundos", "seconds", "s"].contains(unit)) {
            return Err(error("target duration must be expressed in seconds"));
        }
        let v = tokens.first().and_then(|w| w.parse::<f64>().ok()).ok_or_else(|| error("specify a duration in seconds"))?;
        if v < 1.0 {
            return Err(error("target must be at least one second"));
        }
        let t = seconds(v)?;
        let r = q.frame_rate();
        let snap = r.snap(t);
        Some(if snap > t { snap - r.frame_duration() } else { snap })
    } else {
        None
    };
    if target.is_none()
        && ![
            "corta los silencios",
            "corta silencios",
            "elimina silencios",
            "remove silence",
            "remove silences",
            "cut silence",
            "cut silences",
            "hace este video mas corto",
            "make this video shorter",
        ]
        .contains(&n.as_str())
    {
        return Err(error(
            "supported: corta los silencios; busca <text>; version de <seconds> segundos. Local analysis cannot judge the best moments or topics",
        ));
    }
    if !q.audio_tracks.iter().any(|t| !t.muted && t.items.iter().any(|i| i.enabled)) {
        return Err(error("the sequence has no enabled audio to analyse"));
    }
    let sr = q.settings.sample_rate;
    if !(8000..=192000).contains(&sr) {
        return Err(error("audio analysis requires a sample rate between 8000 and 192000 Hz"));
    }
    let mut provider = s.media.provider(s.project.clone(), s.services.clone());
    provider.full_res = true;
    s.assistant.work = Some(Analysis {
        project: s.project.clone(),
        sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        provider: CheckedProvider { inner: provider, error: Arc::default() },
        sample: 0,
        total: q.duration().to_units_floor(i64::from(sr)),
        silence_start: None,
        ranges: Vec::new(),
        threshold: number(p, "thresholdDb", -40.0, -96.0, -6.0)?,
        min: seconds(number(p, "minSeconds", 0.4, 0.1, 30.0)?)?,
        keep: seconds(number(p, "keepSeconds", 0.1, 0.0, 1.0)?)?,
        target,
        started: web_time::Instant::now(),
    });
    inspect(s, &Value::Null)
}

fn push_silence(w: &mut Analysis, end: i64) {
    let Some(start) = w.silence_start.take() else { return };
    let Some(q) = w.project.sequence(w.sequence) else { return };
    let sr = i64::from(q.settings.sample_rate);
    let (a, b) = (Tick::from_units(start, sr), Tick::from_units(end, sr));
    if b - a < w.min {
        return;
    }
    let rate = q.frame_rate();
    let fd = rate.frame_duration();
    let a = a + w.keep;
    let b = b - w.keep;
    let aa = rate.snap(a);
    let aa = if aa < a { aa + fd } else { aa };
    let bb = rate.snap(b);
    let bb = if bb > b { bb - fd } else { bb };
    if bb > aa {
        w.ranges.push(TimeRange::from_bounds(aa, bb));
    }
}

/// Advance at most 200 ms of audio per UI pass. No raw audio accumulates across passes.
pub fn poll(s: &mut Session) {
    let Some(mut w) = s.assistant.work.take() else { return };
    if !Arc::ptr_eq(&w.project, &s.project) || s.state.active_sequence != Some(w.sequence) {
        s.assistant.error = Some("The project changed during analysis. Generate a new preview.".into());
        return;
    }
    let snapshot = w.project.clone();
    let Some(q) = snapshot.sequence(w.sequence) else { return };
    let sr = q.settings.sample_rate;
    let frames = (w.total - w.sample).min((i64::from(sr) / 5).max(1)).max(0) as usize;
    filmcraft_media::pending::take();
    let audio = filmcraft_render::audio::mix_sequence(&w.project, q, w.sample, frames, &w.provider);
    if filmcraft_media::pending::take() {
        w.provider.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if w.started.elapsed().as_secs() > 30 {
            s.assistant.error = Some("Media data did not become available within 30 seconds.".into());
        } else {
            s.assistant.work = Some(w);
        }
        return;
    }
    let failure = w.provider.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(e) = failure {
        s.assistant.error = Some(e);
        return;
    }
    if audio.channels.iter().flatten().any(|v| !v.is_finite()) {
        s.assistant.error = Some("The audio mix contains nonfinite samples.".into());
        return;
    }
    let window = (sr / 50).max(1) as usize;
    for offset in (0..frames).step_by(window) {
        let end = (offset + window).min(frames);
        let mut energy = 0.0f64;
        for ch in &audio.channels {
            let Some(samples) = ch.get(offset..end) else {
                s.assistant.error = Some("Incomplete audio buffer.".into());
                return;
            };
            let e = samples.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (end - offset).max(1) as f64;
            energy = energy.max(e);
        }
        let at = w.sample + offset as i64;
        if 10.0 * energy.max(1e-12).log10() <= w.threshold {
            w.silence_start.get_or_insert(at);
        } else {
            push_silence(&mut w, at);
        }
    }
    w.sample += frames as i64;
    w.started = web_time::Instant::now();
    if w.ranges.len() > MAX_ACTIONS {
        s.assistant.error = Some("More than 256 cuts: analyse a shorter sequence or increase minimum silence.".into());
        return;
    }
    if w.sample < w.total {
        s.assistant.work = Some(w);
        return;
    }
    let duration = q.duration();
    let rate = q.frame_rate();
    let end = w.total;
    push_silence(&mut w, end);
    if let Some(target) = w.target {
        // Retain non-silent material in its original order up to the requested duration.
        let mut remaining = target;
        let mut cursor = Tick::ZERO;
        for r in &w.ranges {
            let span = r.start - cursor;
            if span >= remaining {
                break;
            }
            remaining -= span;
            cursor = r.end();
        }
        let cut = rate.snap(cursor + remaining).min(duration);
        if cut < duration {
            w.ranges.push(TimeRange::from_bounds(cut, duration));
        }
    }
    let label = match w.target {
        Some(t) => format!("Remove quiet intervals; retain chronological material up to {:.2} seconds", t.seconds()),
        None => "Remove quiet intervals from the audible mix (RMS analysis, not speech recognition)".into(),
    };
    if let Err(e) = store(s, w.ranges, None, label) {
        s.assistant.error = Some(e.to_string());
    }
}

fn inspect(s: &mut Session, _: &Value) -> Result<Value> {
    if s.assistant.work.is_some() {
        poll(s);
    }
    let work = s.assistant.work.as_ref();
    let plan = s.assistant.plan.as_ref();
    Ok(json!({
        "analysing":work.is_some(), "progress":work.map(|w| w.sample as f64 / w.total.max(1) as f64), "error":s.assistant.error,
        "plan":plan.map(|p| json!({"actions":p.actions,"token":p.token,"label":p.label,"sequence":p.sequence.0,"stale":!Arc::ptr_eq(&p.project,&s.project)||s.state.active_sequence!=Some(p.sequence),
            "ranges":p.ranges.iter().map(|r| json!({"start":r.start.0,"end":r.end().0})).collect::<Vec<_>>(), "seek":p.seek.map(|t|t.0), "matches":p.matches.iter().map(|r|json!({"start":r.start.0,"end":r.end().0})).collect::<Vec<_>>(),
            "removedSeconds":p.ranges.iter().map(|r|r.duration.seconds()).sum::<f64>(),
            "outputSeconds":p.result.as_ref().unwrap_or(&p.project).sequence(p.sequence).map(|q|q.duration().seconds()-p.ranges.iter().map(|r|r.duration.seconds()).sum::<f64>())}))
    }))
}

fn context(s: &mut Session, _: &Value) -> Result<Value> {
    checked_references(s)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    Ok(json!({"schemaVersion":1,"sequence":s.state.active_sequence.map(|i|i.0),"durationSeconds":q.duration().seconds(),"fps":q.frame_rate().as_f64(),
        "clips":q.all_tracks().flat_map(|t| t.items.iter().map(move |i|json!({"id":i.id.0,"track":t.id.0,"name":i.name.chars().take(256).collect::<String>(),"kind":format!("{:?}",t.kind),"gainDb":i.gain_db,"startSeconds":i.start.seconds(),"endSeconds":i.end().seconds(),"locked":t.locked}))).take(1000).collect::<Vec<_>>(),
        "words":crate::transcript::sequence_words(s).into_iter().take(10000).map(|w|json!({"text":w.text.chars().take(256).collect::<String>(),"startSeconds":w.start.seconds(),"endSeconds":w.end.seconds(),"confidence":w.confidence})).collect::<Vec<_>>()
    }))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let token = p.get("token").and_then(Value::as_u64).ok_or_else(|| error("review token is required"))?;
    let plan = s.assistant.plan.as_ref().ok_or_else(|| error("generate and review a preview first"))?;
    if plan.token != token || !Arc::ptr_eq(&plan.project, &s.project) || s.state.active_sequence != Some(plan.sequence) {
        return Err(error("proposal is stale; generate a new preview"));
    }
    editable(s, &plan.ranges)?;
    let ranges = plan.ranges.clone();
    let seek = plan.seek;
    let edit_count = plan.actions.len();
    if let Some(result) = plan.result.clone() {
        s.edit("Assistant Clip Edits", move |p, st| {
            *p = (*result).clone();
            st.selection.clear();
            Ok(())
        })?;
    }
    if !ranges.is_empty() {
        s.edit_sequence("Assistant Edit", |q, ctx, st| {
            let tracks = q.all_tracks().map(|t| t.id).collect::<Vec<_>>();
            for r in ranges.iter().rev() {
                filmcraft_edit::extract(q, &tracks, *r, ctx);
                if st.ripple_sequence_markers {
                    crate::sequence_tools::ripple_markers(&mut q.markers, r.end(), -r.duration);
                }
            }
            q.mark_in = None;
            q.mark_out = None;
            q.split = Default::default();
            st.selection.clear();
            Ok(())
        })?;
        s.set_playhead(s.playhead().min(s.active_sequence().map(|q| q.duration()).unwrap_or_default()));
    }
    if let Some(time) = seek {
        s.execute("playhead.set", json!({"time":time.0}))?;
    }
    s.assistant.plan = None;
    Ok(json!({"cuts":ranges.len(),"edits":edit_count,"seek":seek.map(|t|t.0)}))
}

/// Reuse ordinary commands in an isolated session. Preview and failures cannot mutate the live
/// project; adoption after token validation creates one undo snapshot for the complete plan.
fn stage_edits(s: &Session, actions: &[Action]) -> Result<Arc<Project>> {
    use filmcraft_project::{ClipId, TrackId, TrackKind};
    checked_references(s)?;
    let mut stage = Session::new(s.services.clone());
    stage.project = s.project.clone();
    stage.state = s.state.clone();
    stage.media = s.media.clone();
    stage.prefs = s.prefs.clone();
    for action in actions {
        let q = stage.active_sequence().ok_or(EngineError::NoSequence)?;
        let rate = q.frame_rate();
        let clip = match action {
            Action::Split { clip, .. } | Action::Move { clip, .. } | Action::Trim { clip, .. } | Action::Gain { clip, .. } => ClipId(*clip),
            _ => return Err(error("invalid clip action")),
        };
        let (tr, it) = q.find_item(clip).ok_or_else(|| error("clip id is not in the active sequence"))?;
        let track = q.track(tr).ok_or_else(|| error("clip track is missing"))?;
        let kind = track.kind;
        if track.locked {
            return Err(error("unlock the target clip track before editing"));
        }
        let (cmd, params) = match action {
            Action::Split { time, .. } => {
                let t = rate.snap(seconds(*time)?);
                if t <= it.start || t >= it.end() {
                    return Err(error("split must be inside the clip"));
                }
                ("timeline.razor", json!({"clip":clip.0,"time":t.0}))
            }
            Action::Move { start, track, .. } => {
                let at = rate.snap(seconds(*start)?);
                let dest = q.track(TrackId(*track)).ok_or_else(|| error("destination track does not exist"))?;
                if dest.locked || dest.kind != kind {
                    return Err(error("move requires an unlocked compatible track"));
                }
                if at + it.duration > seconds(MAX_SECONDS)? {
                    return Err(error("moved clip exceeds the planning range"));
                }
                if dest.items.iter().any(|c| c.id != clip && c.start < at + it.duration && c.end() > at) {
                    return Err(error("move would overwrite another clip; choose a free span"));
                }
                ("timeline.move", json!({"moves":[{"clip":clip.0,"track":track,"time":at.0}],"insert":false,"linked":true}))
            }
            Action::Trim { edge, delta, .. } => {
                if !["in", "out"].contains(&edge.as_str()) || !delta.is_finite() || !(-600.0..=600.0).contains(delta) {
                    return Err(error("trim requires in/out and a finite delta within 600 seconds"));
                }
                ("timeline.trim", json!({"clip":clip.0,"edge":edge,"mode":"regular","delta":rate.snap(Tick::from_seconds_f64(*delta)).0}))
            }
            Action::Gain { db, .. } => {
                if kind != TrackKind::Audio || !db.is_finite() || !(-96.0..=24.0).contains(db) {
                    return Err(error("gain requires an audio clip and -96 to +24 dB"));
                }
                ("clip.audioGain", json!({"clips":[clip.0],"db":db,"mode":"set"}))
            }
            _ => return Err(error("invalid clip action")),
        };
        let requested_delta = params.get("delta").and_then(Value::as_i64);
        let result = stage.execute(cmd, params)?;
        if cmd == "timeline.trim" && result["delta"].as_i64() != requested_delta {
            return Err(error("trim exceeds source or neighbour bounds; request a smaller delta"));
        }
        if cmd == "timeline.move" && result["overwritten"].as_array().is_some_and(|a| !a.is_empty()) {
            return Err(error("linked move would overwrite other clips; choose a free span"));
        }
        stage.active_sequence().ok_or(EngineError::NoSequence)?.check().map_err(error)?;
        stage.history.undo.clear();
        stage.history.redo.clear();
    }
    Ok(stage.project.clone())
}

fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    s.assistant.work = None;
    s.assistant.plan = None;
    s.assistant.error = None;
    Ok(Value::Null)
}

/// Observe decode failures before the playback mixer converts them into silence. An offline or
/// broken source must never be mistaken for a quiet interval that is safe to delete.
struct CheckedProvider {
    inner: crate::media_pool::PoolProvider,
    error: Arc<Mutex<Option<String>>>,
}
impl SourceProvider for CheckedProvider {
    fn source(&self, item: ItemId) -> Option<SharedSource> {
        let source = self.inner.project.resolve_media(item).filter(|(_, m, _)| !m.offline).and_then(|(root, _, _)| self.inner.source(root));
        match source {
            Some(inner) => Some(Arc::new(CheckedSource { inner, error: self.error.clone() })),
            None => {
                *self.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(format!("Audio source {} is missing or offline; relink it before analysis", item.0));
                None
            }
        }
    }
}
struct CheckedSource {
    inner: SharedSource,
    error: Arc<Mutex<Option<String>>>,
}
impl MediaSource for CheckedSource {
    fn info(&self) -> &filmcraft_media::MediaInfo {
        self.inner.info()
    }
    fn video_frame(&self, req: filmcraft_media::FrameRequest) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
        self.inner.video_frame(req)
    }
    fn audio(&self, start: i64, frames: usize, rate: u32) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
        let result = self.inner.audio(start, frames, rate).and_then(|b| {
            if b.channels.iter().flatten().any(|v| !v.is_finite()) { Err(filmcraft_media::MediaError::Decode("nonfinite audio samples".into())) } else { Ok(b) }
        });
        if let Err(e) = &result {
            *self.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!("Audio analysis failed: {e}"));
        }
        result
    }
}

#[cfg(test)]
#[path = "assistant_tests.rs"]
mod tests;
