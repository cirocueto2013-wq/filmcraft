//! Original synthetic ES/EN seed examples for evaluating a restricted editing planner. This is
//! not a trained model or a replacement for real reviewed edits. No media or credentials uploaded.
use filmcraft_engine::Session;
use serde_json::json;
use std::{io::Write, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).ok_or("usage: cargo run -p filmcraft-engine --example editor_dataset -- OUTPUT_DIR")?;
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir)?;
    let mut train = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("train.jsonl"))?;
    let mut evaluation = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("eval.jsonl"))?;
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({}))?;
    let mut count = [0usize; 2];
    for scenario in 0..60 {
        // Split by scenario BEFORE translating: an ES/EN pair is never on opposite sides.
        let holdout = scenario % 5 == 0;
        let a = 0.25 + scenario as f64 * 0.09;
        let b = a + 0.5;
        for language in ["es", "en"] {
            let instruction = if language == "es" {
                format!("Eliminá el intervalo entre {a:.3} y {b:.3} segundos de todas las pistas.")
            } else {
                format!("Remove the interval from {a:.3} to {b:.3} seconds across all tracks.")
            };
            s.execute("assistant.plan", json!({"schemaVersion":1,"actions":[{"type":"extract","start":a,"end":b}]}))?;
            let row = s.execute("ai.trainingExample", json!({"instruction":instruction}))?;
            let out = if holdout {
                count[1] += 1;
                &mut evaluation
            } else {
                count[0] += 1;
                &mut train
            };
            writeln!(out, "{row}")?;
        }
    }
    train.sync_all()?;
    evaluation.sync_all()?;
    println!("{} training / {} held-out rows; original synthetic examples, not trained weights", count[0], count[1]);
    Ok(())
}
