use super::*;
use filmcraft_project::{Transcript, Word};

fn session() -> (Session, ItemId, std::path::PathBuf) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"width":160,"height":90,"fps":24,"video":1,"audio":1})).unwrap();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let suffix = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("filmcraft-assistant-{}-{}.wav", std::process::id(), suffix));
    let samples: Vec<f32> = (0..48000 * 4)
        .map(|i| if (48000..96000).contains(&i) || i >= 144000 { 0.2 * (i as f32 * std::f32::consts::TAU * 440.0 / 48000.0).sin() } else { 0.0 })
        .collect();
    std::fs::write(&path, filmcraft_media::wav::write_wav16(&samples, 1, 48000)).unwrap();
    let item = ItemId(s.execute("file.import", json!({"paths":[path]})).unwrap()["items"][0].as_u64().unwrap());
    s.execute("timeline.place", json!({"item":item.0,"track":"A1","seconds":0,"duration":Tick::from_seconds_f64(4.0).0})).unwrap();
    (s, item, path)
}

fn complete(s: &mut Session) -> Value {
    for _ in 0..100 {
        poll(s);
        if s.assistant.work.is_none() {
            return inspect(s, &Value::Null).unwrap();
        }
    }
    panic!("analysis did not complete")
}

#[test]
fn audio_analysis_preview_is_pure_apply_is_one_undo_and_redo() {
    let (mut s, _, path) = session();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    s.execute("assistant.plan", json!({"instruction":"cortá los silencios"})).unwrap();
    let state = complete(&mut s);
    assert!(state["error"].is_null(), "{state}");
    let p = &state["plan"];
    assert_eq!(p["ranges"].as_array().unwrap().len(), 2);
    assert_eq!(p["removedSeconds"].as_f64().unwrap(), 1.5);
    assert!(Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.history.undo.len(), undo);
    s.execute("assistant.apply", json!({"token":p["token"]})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = s.project.clone();
    assert_eq!(s.active_sequence().unwrap().duration().seconds(), 2.5);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*s.project, *after);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn hostile_external_actions_stale_tokens_and_project_replacement_are_rejected() {
    let (mut s, _, path) = session();
    let before = s.project.clone();
    for value in [
        json!({"actions":[{"type":"execute","command":"file.delete"}]}),
        json!({"actions":[{"type":"extract","start":-1,"end":2}]}),
        json!({"actions":[{"type":"extract","start":0,"end":999}]}),
        json!({"actions":[{"type":"extract","start":2,"end":1}]}),
        json!({"actions":[{"type":"seek","time":99}]}),
        json!({"actions":"garbage"}),
        json!({"instruction":"elige las mejores partes"}),
        json!({"instruction":"corta silencios","thresholdDb":"garbage"}),
    ] {
        assert!(s.execute("assistant.plan", value).is_err());
        assert_eq!(*s.project, *before);
        assert!(s.assistant.plan.is_none());
        assert!(s.assistant.work.is_none());
    }
    let p = s.execute("assistant.plan", json!({"actions":[{"type":"extract","start":1,"end":2}]})).unwrap();
    s.execute("playhead.set", json!({"seconds":3})).unwrap(); // Navigation keeps the review valid.
    s.execute("file.newProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"width":160,"height":90})).unwrap();
    assert!(s.execute("assistant.apply", json!({"token":p["plan"]["token"]})).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn search_uses_real_timestamps_and_locked_tracks_stop_ripple_plans() {
    let (mut s, item, path) = session();
    let t = Transcript {
        words: vec![
            Word::new("Hola", Tick::from_seconds_f64(1.0), Tick::from_seconds_f64(1.3)),
            Word::new("mundo", Tick::from_seconds_f64(1.3), Tick::from_seconds_f64(1.7)),
        ],
        ..Default::default()
    };
    s.execute("transcript.set", json!({"item":item.0,"transcript":t})).unwrap();
    let result = s.execute("assistant.plan", json!({"instruction":"busca donde digo hola mundo"})).unwrap();
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    let token = result["state"]["plan"]["token"].clone();
    s.execute("assistant.apply", json!({"token":token})).unwrap();
    assert_eq!(s.playhead(), Tick::from_seconds_f64(1.0));
    s.execute("timeline.setTrack", json!({"track":"A1","locked":true})).unwrap();
    assert!(s.execute("assistant.plan", json!({"actions":[{"type":"extract","start":1,"end":2}]})).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cancelling_analysis_and_edits_during_analysis_never_change_the_project() {
    let (mut s, _, path) = session();
    let before = s.project.clone();
    s.execute("assistant.plan", json!({"instruction":"remove silence"})).unwrap();
    poll(&mut s);
    s.execute("assistant.cancel", json!({})).unwrap();
    assert!(s.assistant.work.is_none());
    assert_eq!(*s.project, *before);
    s.execute("assistant.plan", json!({"instruction":"remove silence"})).unwrap();
    s.execute("sequence.settings", json!({"width":180})).unwrap();
    let changed = s.project.clone();
    poll(&mut s);
    assert!(s.assistant.error.is_some());
    assert_eq!(*s.project, *changed);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn target_duration_offline_audio_and_schema_limits() {
    let (mut s, item, path) = session();
    s.execute("assistant.plan", json!({"instruction":"version de 2 segundos"})).unwrap();
    let state = complete(&mut s);
    assert!(state["error"].is_null(), "{state}");
    s.execute("assistant.apply", json!({"token":state["plan"]["token"]})).unwrap();
    assert!(s.active_sequence().unwrap().duration().seconds() <= 2.0);
    s.execute("edit.undo", json!({})).unwrap();
    let too_many = vec![json!({"type":"extract","start":1,"end":2}); 257];
    assert!(s.execute("assistant.plan", json!({"actions":too_many})).is_err());
    s.edit("offline", |p, _| {
        let filmcraft_project::ItemKind::Media(m) = &mut p.item_mut(item).unwrap().kind else { panic!() };
        m.offline = true;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    s.execute("assistant.plan", json!({"instruction":"remove silences"})).unwrap();
    let result = complete(&mut s);
    assert!(result["error"].as_str().unwrap().contains("offline"));
    assert!(result["plan"].is_null());
    assert_eq!(*s.project, *before);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn clip_plan_previews_split_trim_gain_and_move_as_one_atomic_edit() {
    let (mut s, _, path) = session();
    let clip = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    let track = s.active_sequence().unwrap().audio_tracks[0].id;
    let before = s.project.clone();
    let history = s.history.undo.len();
    let preview = s
        .execute(
            "assistant.plan",
            json!({"schemaVersion":1,"actions":[
                {"type":"gain","clip":clip.0,"db":-6.0},
                {"type":"trim","clip":clip.0,"edge":"out","delta":-1.0},
                {"type":"move","clip":clip.0,"start":1.0,"track":track.0},
                {"type":"split","clip":clip.0,"time":2.0}
            ]}),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.history.undo.len(), history);
    assert_eq!(preview["plan"]["actions"].as_array().unwrap().len(), 4);
    let row = s
        .execute("ai.trainingExample", json!({"instruction":"Reduce audio by six dB, shorten by one second, move to one second and split at two seconds"}))
        .unwrap();
    let actions: Value = serde_json::from_str(row["messages"][2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(actions["actions"].as_array().unwrap().len(), 4);
    s.execute("assistant.apply", json!({"token":preview["plan"]["token"]})).unwrap();
    assert_eq!(s.history.undo.len(), history + 1);
    let audio = &s.active_sequence().unwrap().audio_tracks[0].items;
    assert_eq!(audio.len(), 2);
    assert!(audio.iter().all(|i| i.gain_db == -6.0));
    assert_eq!(audio[0].start.seconds(), 1.0);
    assert_eq!(audio[0].end().seconds(), 2.0);
    assert_eq!(audio[1].end().seconds(), 4.0);
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    assert_eq!(s.active_sequence().unwrap().audio_tracks[0].items.len(), 2);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn invalid_clip_actions_cannot_partially_edit_or_leave_a_proposal_armed() {
    let (mut s, _, path) = session();
    let clip = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    let before = s.project.clone();
    for action in [
        json!({"type":"gain","clip":clip.0,"db":100.0}),
        json!({"type":"gain","clip":u64::MAX,"db":-6}),
        json!({"type":"split","clip":clip.0,"time":4.0}),
        json!({"type":"trim","clip":clip.0,"edge":"unknown","delta":1}),
        json!({"type":"trim","clip":clip.0,"edge":"out","delta":-600}),
        json!({"type":"move","clip":clip.0,"start":1,"track":u64::MAX}),
    ] {
        assert!(s.execute("assistant.plan", json!({"actions":[{"type":"gain","clip":clip.0,"db":-6},action]})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert!(s.execute("assistant.inspect", json!({})).unwrap()["plan"].is_null());
    }
    std::fs::remove_file(path).unwrap();
}
