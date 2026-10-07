//! The export audio path: the sequence mix at the output sample rate, folded to the output
//! channel count, with Export ▸ Effects ▸ Loudness Normalization (a BS.1770-4 measuring pass over
//! the export range, then a linear gain and a look-ahead true-peak limiter).
//!
//! The limiter's look-ahead delays its output; [`AudioOut`] mixes that many samples ahead (silence
//! past the end of the range) and drops the first ones, so the exported audio stays aligned with
//! the picture and exactly as long as the range.

use std::sync::Arc;

use filmcraft_audio_dsp::AudioEffect;
use filmcraft_audio_dsp::effects::Limiter;
use filmcraft_project::{ItemId, Project, Sequence};
use filmcraft_render::SourceProvider;
use filmcraft_time::TimeRange;

use crate::{ExportError, ExportSettings, Result};

/// What loudness normalization measured and did.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct LoudnessReport {
    pub measured_lufs: f64,
    pub gain_db: f64,
}

pub(crate) struct AudioOut {
    project: Arc<Project>,
    /// The sequence with the output sample rate.
    seq: Sequence,
    pub sr: u32,
    pub channels: usize,
    gain: f32,
    limiter: Option<Limiter>,
    latency: i64,
    /// Next sample to mix (runs `latency` ahead of `out`).
    next_in: i64,
    /// Next sample handed out.
    pub out: i64,
    /// End of the range (samples); input past it is silence.
    end: i64,
    pub loudness: Option<LoudnessReport>,
}

impl AudioOut {
    pub fn new(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, range: TimeRange) -> Result<Self> {
        let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
        let r = settings.resolve(q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate);
        let mut seq = q.clone();
        seq.settings.sample_rate = r.sample_rate;
        let sr = r.sample_rate;
        let start = range.start.to_units_floor(sr as i64);
        let end = range.end().to_units_floor(sr as i64);
        let ln = &settings.effects.loudness;
        let limiter = ln.enabled.then(|| {
            let mut l = Limiter::new(sr as f32, r.channels as usize);
            l.set_param("ceiling", ln.true_peak_dbtp.clamp(-24.0, 0.0) as f32);
            l.set_param("true_peak", 1.0);
            l.set_param("release", 80.0);
            l.reset();
            l
        });
        let latency = limiter.as_ref().map(|l| l.latency() as i64).unwrap_or(0);
        Ok(AudioOut { project, seq, sr, channels: r.channels as usize, gain: 1.0, limiter, latency, next_in: start, out: start, end, loudness: None })
    }

    /// The loudness pass: measure the integrated loudness of the whole range and set the gain
    /// that brings it to the target. `cancel` is polled between chunks.
    pub fn measure(&mut self, settings: &ExportSettings, sources: &dyn SourceProvider, cancel: &dyn Fn() -> bool) -> Result<()> {
        let ln = &settings.effects.loudness;
        if !ln.enabled {
            return Ok(());
        }
        let mut meter = filmcraft_audio_dsp::LoudnessMeter::new(self.sr as f64, self.channels);
        let mut pos = self.out;
        while pos < self.end {
            if cancel() {
                return Err(ExportError::Cancelled);
            }
            let n = (self.end - pos).min(self.sr as i64) as usize;
            let buf = self.mix(pos, n, sources);
            if filmcraft_media::pending::is_set() {
                return Ok(());
            }
            let refs: Vec<&[f32]> = buf.iter().map(Vec::as_slice).collect();
            meter.process(&refs);
            pos += n as i64;
        }
        let measured = meter.integrated();
        let gain_db = filmcraft_audio_dsp::normalize_gain_db(measured, ln.target_lufs);
        self.gain = 10f64.powf(gain_db / 20.0) as f32;
        self.loudness = Some(LoudnessReport { measured_lufs: measured, gain_db });
        Ok(())
    }

    /// Mix `n` samples from `pos`, folded to the output channels (silence past the range end).
    /// Stereo output of a 5.1 Mix is the ITU-R BS.775 downmix; 5.1 output of a stereo Mix places it
    /// on L and R; 5.1 is in L, R, C, LFE, Ls, Rs order.
    fn mix(&self, pos: i64, n: usize, sources: &dyn SourceProvider) -> Vec<Vec<f32>> {
        let real = (self.end - pos).clamp(0, n as i64) as usize;
        let out_ch = if self.channels == 6 { 6 } else { 2 };
        let mut st = if real > 0 {
            let layout = filmcraft_audio_dsp::channels::Layout::from_channels(out_ch);
            filmcraft_render::audio::mix_sequence_layout(
                &self.project,
                &self.seq,
                pos,
                real,
                sources,
                layout,
                filmcraft_audio_dsp::channels::Mixdown::FrontRear,
            )
            .channels
        } else {
            vec![Vec::new(); out_ch]
        };
        st.resize(out_ch, Vec::new());
        for c in st.iter_mut() {
            c.resize(n, 0.0);
        }
        if self.channels == 1 { vec![st[0].iter().zip(&st[1]).map(|(l, r)| 0.5 * (l + r)).collect()] } else { st }
    }

    /// Output samples up to `until` (exclusive), planar; None when there is nothing new.
    pub fn pull(&mut self, until: i64, sources: &dyn SourceProvider) -> Option<Vec<Vec<f32>>> {
        let until = until.min(self.end);
        if until <= self.out {
            return None;
        }
        let want_in = until + self.latency;
        let n_in = (want_in - self.next_in).max(0) as usize;
        let mut buf = self.mix(self.next_in, n_in, sources);
        // A browser read may have returned an incomplete mix. Retry without consuming samples
        // or advancing the limiter's delay line; the exporter observes the same pending flag.
        if filmcraft_media::pending::is_set() {
            return None;
        }
        // the first call also fills the limiter's look-ahead
        let drop = (self.latency - (self.next_in - self.out)).clamp(0, n_in as i64) as usize;
        self.next_in = want_in;
        if self.gain != 1.0 {
            buf.iter_mut().for_each(|c| c.iter_mut().for_each(|s| *s *= self.gain));
        }
        if let Some(l) = self.limiter.as_mut() {
            let mut refs: Vec<&mut [f32]> = buf.iter_mut().map(Vec::as_mut_slice).collect();
            l.process(&mut refs);
        }
        let n_out = (until - self.out) as usize;
        for c in buf.iter_mut() {
            c.drain(..drop.min(c.len()));
            c.resize(n_out, 0.0);
        }
        self.out = until;
        Some(buf)
    }

    /// Mix a whole batch transactionally, then partition it at the mux interleave boundaries.
    /// Even a batch spanning several groups must not consume an earlier group if a later read
    /// defers. Boundaries supplied by the exporter are increasing sample positions.
    pub fn pull_groups(&mut self, boundaries: &[i64], sources: &dyn SourceProvider) -> Vec<Option<Vec<Vec<f32>>>> {
        let mut pos = self.out;
        let Some(&until) = boundaries.last() else { return Vec::new() };
        let Some(planar) = self.pull(until, sources) else { return boundaries.iter().map(|_| None).collect() };
        if boundaries.len() == 1 {
            return vec![Some(planar)];
        }
        let mut offset = 0;
        boundaries
            .iter()
            .map(|&boundary| {
                let end = boundary.min(self.end).max(pos);
                let n = (end - pos) as usize;
                pos = end;
                // pull produced exactly `until - old_out` samples on every channel, and these
                // increasing, end-clamped boundaries partition that same interval.
                let chunk = planar.iter().map(|c| c[offset..offset + n].to_vec()).collect();
                offset += n;
                (n > 0).then_some(chunk)
            })
            .collect()
    }

    /// Everything still to come (to the end of the range).
    pub fn rest(&mut self, sources: &dyn SourceProvider) -> Option<Vec<Vec<f32>>> {
        self.pull(self.end, sources)
    }
}

/// Interleave planar samples.
pub(crate) fn interleave(planar: &[Vec<f32>]) -> Vec<f32> {
    let n = planar.first().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(n * planar.len());
    for i in 0..n {
        for c in planar {
            out.push(c[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_frame::{AudioBuffer, VideoFrame};
    use filmcraft_media::{FrameRequest, MediaInfo, MediaSource, SharedSource};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct DeferredAudio {
        source: SharedSource,
        defer: AtomicBool,
    }

    impl MediaSource for DeferredAudio {
        fn info(&self) -> &MediaInfo {
            self.source.info()
        }
        fn video_frame(&self, req: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
            self.source.video_frame(req)
        }
        fn audio(&self, start: i64, frames: usize, sr: u32) -> filmcraft_media::Result<AudioBuffer> {
            if self.defer.swap(false, Ordering::Relaxed) {
                return Err(filmcraft_media::MediaError::Io(filmcraft_media::pending::would_block().to_string()));
            }
            self.source.audio(start, frames, sr)
        }
    }

    #[test]
    fn deferred_reads_preserve_audio_and_limiter_state() {
        for normalize in [false, true] {
            let _ = filmcraft_media::pending::take();
            let (project, seq, mut sources) = crate::tests::project();
            let id = project.sequence(seq).unwrap().audio_tracks[0].items[0].item;
            let source = sources.0.get(&id).unwrap().clone();
            let mut settings = ExportSettings::default();
            settings.effects.loudness.enabled = normalize;
            let range = TimeRange::new(filmcraft_time::Tick::ZERO, filmcraft_time::FrameRate::FPS_24.tick_of(24));
            let mut expected = AudioOut::new(project.clone(), seq, &settings, range).unwrap();
            let mut actual = AudioOut::new(project, seq, &settings, range).unwrap();
            let wanted = expected.pull(32_000, &sources).unwrap();
            sources.0.insert(id, Arc::new(DeferredAudio { source, defer: AtomicBool::new(true) }));
            assert!(actual.pull(32_000, &sources).is_none());
            assert_eq!(actual.out, 0);
            assert!(filmcraft_media::pending::take());
            assert_eq!(actual.pull(32_000, &sources).unwrap(), wanted);
            assert_eq!(actual.rest(&sources).unwrap(), expected.rest(&sources).unwrap());
        }
    }

    #[test]
    fn interleave_groups_retry_as_one_audio_transaction() {
        let _ = filmcraft_media::pending::take();
        let (project, seq, mut sources) = crate::tests::project();
        let id = project.sequence(seq).unwrap().audio_tracks[0].items[0].item;
        let source = sources.0.get(&id).unwrap().clone();
        let mut settings = ExportSettings::default();
        settings.effects.loudness.enabled = true;
        let range = TimeRange::new(filmcraft_time::Tick::ZERO, filmcraft_time::FrameRate::FPS_24.tick_of(24));
        let mut expected = AudioOut::new(project.clone(), seq, &settings, range).unwrap();
        let mut actual = AudioOut::new(project, seq, &settings, range).unwrap();
        let boundaries = [16_000, 32_000];
        let wanted = expected.pull_groups(&boundaries, &sources);
        sources.0.insert(id, Arc::new(DeferredAudio { source, defer: AtomicBool::new(true) }));
        assert!(actual.pull_groups(&boundaries, &sources).iter().all(Option::is_none));
        assert_eq!(actual.out, 0);
        assert!(filmcraft_media::pending::take());
        assert_eq!(actual.pull_groups(&boundaries, &sources), wanted);
        assert_eq!(actual.rest(&sources).unwrap(), expected.rest(&sources).unwrap());
    }
}
