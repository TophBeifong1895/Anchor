//! `anchor-eval record --state DIR --config DIGEST < grade.json`
//!
//! Records one grade (an `anchor_facts::Grade` as JSON on stdin) into
//! `DIR/kernel-grades` and prints the verdict as JSON. Exits 1 if the grade
//! was not accepted, 2 on a usage or storage error.

use std::io::Read;
use std::path::PathBuf;

const USAGE: &str = "usage: anchor-eval record --state DIR --config DIGEST < grade.json";

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut state, mut config) = (None, None);
    let mut rest = args.iter();
    if rest.next().map(String::as_str) != Some("record") {
        return Err(USAGE.into());
    }
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--state" => state = Some(PathBuf::from(value)),
            "--config" => config = Some(value.clone()),
            _ => return Err(USAGE.into()),
        }
    }
    let (Some(state), Some(config)) = (state, config) else {
        return Err(USAGE.into());
    };
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let grade: anchor_facts::Grade =
        serde_json::from_str(&input).map_err(|e| format!("grade: {e}"))?;
    let recorder = anchor_eval::GradeRecorder::open(&state)?;
    let verdict = recorder
        .record(&grade, &config)
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&verdict).map_err(|e| e.to_string())?
    );
    Ok(verdict.is_accepted())
}
