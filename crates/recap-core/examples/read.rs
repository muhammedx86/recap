//! Developer tool: run a game's screen readers over an image or a video.
//!
//! ```text
//! cargo run --release -p recap-core --example read -- <image.png|video.mp4> <game-folder>
//! ```
//! Text recognition uses the `tesseract` command if it is installed (the app uses Windows OCR).

use std::{io::Write, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use image::GrayImage;
use recap_core::{
    ffmpeg::Ffmpeg,
    games::load_games,
    readers::{read_details, PreparedReader, ReadWindows, TextReader},
};

struct Tesseract;

impl TextReader for Tesseract {
    fn read_text(&self, img: &GrayImage) -> Result<String> {
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        let mut child = Command::new("tesseract")
            .args(["stdin", "stdout", "--psm", "7"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("running tesseract")?;
        child.stdin.take().context("stdin")?.write_all(&png)?;
        let out = child.wait_with_output()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // RECAP_OCR_DUMP=<dir>: save each prepared crop and print what was read.
        if let Some(dir) = std::env::var_os("RECAP_OCR_DUMP") {
            let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
            let _ = img.save(PathBuf::from(dir).join(format!("ocr-{n:03}.png")));
            eprintln!("ocr-{n:03}: {text:?}");
        }
        Ok(text)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "usage: read <image|video> <game-folder>");
    let input = PathBuf::from(&args[1]);
    let game_dir = PathBuf::from(&args[2]);
    let root = game_dir.parent().context("game folder has no parent")?.to_path_buf();
    let id = game_dir.file_name().unwrap().to_string_lossy().to_string();
    let game = load_games(&[root])
        .into_iter()
        .find(|g| g.dir.file_name().is_some_and(|n| n.to_string_lossy() == id))
        .context("no game.json")?;
    let ocr = Tesseract;
    let is_image = matches!(
        input.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "bmp")
    );
    if is_image {
        let img = image::open(&input)?.to_rgb8();
        for r in &game.readers {
            let p = PreparedReader::load(r, img.width(), img.height())?;
            let slot = p.pick_slot(&img);
            let got = p.read_frame(&img, Some(&ocr));
            println!("{:<16} slot {:?}  -> {:?}", r.def.id, slot, got);
        }
    } else {
        let ff = Ffmpeg::locate(&[]);
        let info = ff.probe(&input)?;
        let w = ReadWindows::for_match(info.duration_ms, None, None, None);
        let w = ReadWindows { end: w.any, ..w };
        let d = read_details(&ff, &input, &game.readers, &w, Some(&ocr), None)?;
        for (k, v) in d {
            println!("{k:<12} {} ({:?})", v.value, v.confidence);
        }
    }
    Ok(())
}
