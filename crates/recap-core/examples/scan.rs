//! Developer tool: scan a recording with a game folder's detectors and print what fires.
//!
//!     cargo run --release -p recap-core --example scan -- <video> <game-folder> [analysis-height]
//!
//! Prints each event and, per detector, the peak score while firing and the highest score
//! seen outside events (how close it came to a false positive).

use std::path::PathBuf;

use recap_core::{
    ffmpeg::Ffmpeg,
    games::load_games,
    scan::{scan_video, ScanOptions},
};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "usage: scan <video> <game-folder> [analysis-height]");
    let video = PathBuf::from(&args[1]);
    let game_dir = PathBuf::from(&args[2]);
    let height: u32 = args.get(3).and_then(|h| h.parse().ok()).unwrap_or(720);
    let root = game_dir.parent().map(PathBuf::from).unwrap_or_default();
    let id = game_dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let game = load_games(&[root])
        .into_iter()
        .find(|g| g.dir.file_name().is_some_and(|n| n.to_string_lossy() == id));
    let game = game.ok_or_else(|| anyhow::anyhow!("no game.json in {}", game_dir.display()))?;
    let ff = Ffmpeg::locate(&[]);
    let res = scan_video(
        &ff,
        &video,
        &game.detectors,
        ScanOptions {
            analysis_height: height,
            keep_traces: true,
            ..Default::default()
        },
    )?;
    for e in &res.events {
        println!(
            "{:>8.2}s  {:<12} {:<20} conf {:.3}",
            e.t_ms as f64 / 1000.0,
            e.kind,
            e.source.label(),
            e.confidence.unwrap_or(0.0)
        );
    }
    // RECAP_SCAN_DUMP=1 prints every sample: "<detector> <seconds> <score>".
    if std::env::var_os("RECAP_SCAN_DUMP").is_some() {
        for t in &res.traces {
            for (ms, s) in &t.samples {
                println!("trace {} {:.3} {:.3}", t.detector, *ms as f64 / 1000.0, s);
            }
        }
    }
    for t in &res.traces {
        let fired: Vec<u64> = res
            .events
            .iter()
            .filter(|e| e.source.label() == format!("vision:{}", t.detector))
            .map(|e| e.t_ms)
            .collect();
        let near = |ms: u64| fired.iter().any(|f| ms + 500 >= *f && ms <= f + 2500);
        let off = t.samples.iter().filter(|(ms, _)| !near(*ms)).map(|s| s.1).fold(-1.0f32, f32::max);
        let on = t.samples.iter().filter(|(ms, _)| near(*ms)).map(|s| s.1).fold(-1.0f32, f32::max);
        println!(
            "{:<20} events {:>3}  peak while firing {:.3}  max elsewhere {:.3}",
            t.detector,
            fired.len(),
            on,
            off
        );
    }
    Ok(())
}
