//! Exercise the desktop audio adapter against a real output device (including an ALSA test PCM).
//! Generates distinct left/right tones and checks that the device clock advances.

#[path = "../src/audio.rs"]
mod audio;
#[path = "../src/audio_in.rs"]
mod audio_in;

use std::time::{Duration, Instant};

use filmcraft_engine::settings::AudioHardwarePrefs;
use filmcraft_engine::voiceover::AudioInput;
use filmcraft_ui_egui::AudioOut;

fn main() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--input") {
        let mut input = audio_in::CpalIn::new("");
        let format = input.start("", 48_000)?;
        let mut samples = vec![Vec::new(); usize::from(format.channels)];
        let started = Instant::now();
        while samples.first().map_or(0, Vec::len) < 2400 && started.elapsed() < Duration::from_secs(2) {
            let remaining = 2400 - samples.first().map_or(0, Vec::len);
            for (channel, chunk) in samples.iter_mut().zip(input.read(remaining)) {
                channel.extend(chunk);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        input.stop();
        if let Some(error) = input.error() {
            return Err(error);
        }
        if samples.len() != usize::from(format.channels) || samples.iter().any(|c| c.len() < 2400) {
            return Err("the input device did not capture 50 ms of audio".into());
        }
        let rms: Vec<f32> = samples.iter().map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let amplitude = |channel: &[f32], hz: f64| {
            let (re, im) = channel.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, s)| {
                let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(format.sample_rate);
                (re + f64::from(*s) * phase.cos(), im + f64::from(*s) * phase.sin())
            });
            2.0 * re.hypot(im) / channel.len() as f64
        };
        let tones: Vec<_> = samples.iter().map(|c| [amplitude(c, 440.0), amplitude(c, 880.0)]).collect();
        if std::env::args().any(|arg| arg == "--check-tones") {
            for (i, tone) in tones.iter().enumerate() {
                let expected = usize::from(i != 0);
                if tone[expected] < 0.24 || tone[1 - expected] > 0.01 {
                    return Err(format!("input channel {i} did not preserve its expected test tone: {tone:?}"));
                }
            }
        }
        println!(
            "{}",
            serde_json::json!({"sampleRate":format.sample_rate,"channels":format.channels,"frames":samples[0].len(),"rms":rms,"tones440and880":tones})
        );
        return Ok(());
    }
    let mut output = audio::CpalOut::new();
    output.configure(&AudioHardwarePrefs::default(), None);
    let sample_rate = output.sample_rate();
    let mut cursor = 0_u64;
    let rate = output.start(Box::new(move |samples, channels| {
        let channels = channels.max(1);
        for frame in samples.chunks_mut(channels) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let frequency = if channel == 0 { 440.0 } else { 880.0 };
                let phase = std::f64::consts::TAU * frequency * cursor as f64 / sample_rate as f64;
                *sample = (phase.sin() * 0.25) as f32;
            }
            cursor = cursor.saturating_add(1);
        }
    }))?;
    let started = Instant::now();
    if std::env::args().any(|arg| arg == "--expect-stream-error") {
        while output.played_frames().is_some() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let released = output.played_frames().is_none();
        output.stop();
        if !released {
            return Err("the failed output device retained the playback clock".into());
        }
        println!("{}", serde_json::json!({"clockReleased": true}));
        return Ok(());
    }
    while output.played_frames().unwrap_or(0) < u64::from(rate / 20) && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let frames = output.played_frames().unwrap_or(0);
    output.stop();
    if frames < u64::from(rate / 20) {
        return Err("the output device clock did not advance".into());
    }
    println!("{}", serde_json::json!({"sampleRate": rate, "channels": output.channels(), "frames": frames}));
    Ok(())
}
