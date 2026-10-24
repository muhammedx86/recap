//! A console-subsystem binary on purpose: CI calls it and needs it to block and set the exit code.
//!
//! `recap-sample-game`: run the scripted fake game, or export its fixture/frames/timeline.
//!
//! ```text
//! recap-sample-game [--window-scale <0.25..1>]   run the game (Windows); scale shrinks the window
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
        let fps: f64 = value("--fps")
            .unwrap_or_else(|| "10".into())
            .parse()
            .context("--fps must be a number")?;
        let n = game::export_frames(&dir, fps)?;
        println!("wrote {n} frames to {}", dir.display());
        return Ok(());
    }
    let scale: f64 = match value("--window-scale") {
        Some(v) => v.parse::<f64>().context("--window-scale must be a number")?.clamp(0.25, 1.0),
        None => 1.0,
    };
    if let Some(unknown) = args.iter().find(|a| a.starts_with("--") && *a != "--window-scale") {
        bail!("unknown argument {unknown:?} (try --timeline, --export-fixture <dir>, --export-frames <dir> --fps <n>)");
    }
    run(scale)
}

#[cfg(windows)]
fn run(scale: f64) -> Result<()> {
    use minifb::{Key, Window, WindowOptions};
    use std::time::Instant;

    // On small screens the window is shrunk on purpose (keeping 16:9) rather than letting
    // Windows squeeze it; detectors use fractional regions, so any size works.
    let (w, h) = (
        (game::WIDTH as f64 * scale).round() as usize,
        (game::HEIGHT as f64 * scale).round() as usize,
    );
    let mut window =
        Window::new(game::WINDOW_TITLE, w, h, WindowOptions::default()).map_err(|e| anyhow::anyhow!("could not open the window: {e}"))?;
    // 30 fps is plenty for recording and leaves CPU for the recorder.
    window.set_target_fps(30);
    let mut buf = game::new_frame();
    let mut small = vec![0u32; w * h];
    let start = Instant::now();
    println!(
        "{}: window open ({}x{}), exits after {} s",
        game::WINDOW_TITLE,
        w,
        h,
        game::EXIT_AT_S
    );
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let t = start.elapsed().as_secs_f64();
        if t >= game::EXIT_AT_S {
            break;
        }
        game::render_frame(t, &mut buf);
        let out = if (w, h) == (game::WIDTH, game::HEIGHT) {
            &buf
        } else {
            game::downscale(&buf, &mut small, w, h);
            &small
        };
        window
            .update_with_buffer(out, w, h)
            .map_err(|e| anyhow::anyhow!("window update failed: {e}"))?;
    }
    println!("{}: exiting after {:.1} s", game::WINDOW_TITLE, start.elapsed().as_secs_f64());
    Ok(())
}

#[cfg(not(windows))]
fn run(_scale: f64) -> Result<()> {
    println!("The sample game window only runs on Windows. Use --timeline, --export-fixture or --export-frames here.");
    Ok(())
}
