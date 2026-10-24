//! `recap-sample-game`: run the scripted fake game, or export its fixture/frames/timeline.
//!
//! ```text
//! recap-sample-game                              run the game (Windows)
//! recap-sample-game --export-fixture <dir>       write <dir>/recap-sample-game/ for Recap
//! recap-sample-game --export-frames <dir> --fps <n>
//! recap-sample-game --timeline                   print the banner timeline as JSON
//! ```

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use recap_sample_game as game;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| -> Option<String> { args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned()) };

    if args.iter().any(|a| a == "--timeline") {
        println!("{}", game::timeline_json());
        return Ok(());
    }
    if args.iter().any(|a| a == "--export-fixture") {
        let dir = PathBuf::from(value("--export-fixture").context("--export-fixture needs a folder")?);
        let out = game::export_fixture(&dir)?;
        println!("{}", out.display());
        return Ok(());
    }
    if args.iter().any(|a| a == "--export-frames") {
        let dir = PathBuf::from(value("--export-frames").context("--export-frames needs a folder")?);
        let fps: f64 = value("--fps").unwrap_or_else(|| "10".into()).parse().context("--fps must be a number")?;
        let n = game::export_frames(&dir, fps)?;
        println!("wrote {n} frames to {}", dir.display());
        return Ok(());
    }
    if let Some(unknown) = args.first() {
        bail!("unknown argument {unknown:?} (try --timeline, --export-fixture <dir>, --export-frames <dir> --fps <n>)");
    }
    run()
}

#[cfg(windows)]
fn run() -> Result<()> {
    use minifb::{Key, Window, WindowOptions};
    use std::time::Instant;

    let mut window = Window::new(game::WINDOW_TITLE, game::WIDTH, game::HEIGHT, WindowOptions::default())
        .map_err(|e| anyhow::anyhow!("could not open the window: {e}"))?;
    window.set_target_fps(60);
    let mut buf = game::new_frame();
    let start = Instant::now();
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let t = start.elapsed().as_secs_f64();
        if t >= game::EXIT_AT_S {
            break;
        }
        game::render_frame(t, &mut buf);
        window
            .update_with_buffer(&buf, game::WIDTH, game::HEIGHT)
            .map_err(|e| anyhow::anyhow!("window update failed: {e}"))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn run() -> Result<()> {
    println!("The sample game window only runs on Windows. Use --timeline, --export-fixture or --export-frames here.");
    Ok(())
}
