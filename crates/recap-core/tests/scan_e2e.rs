//! End-to-end: encode a noisy video in which a "kill banner" appears twice, then
//! check the post-match scanner finds exactly those two moments.
//! Skipped when ffmpeg/ffprobe are not installed.

use std::{path::PathBuf, process::Command};

use image::{GrayImage, Luma};
use recap_core::{
    ffmpeg::Ffmpeg,
    games::{DetectMethod, DetectorDef, LoadedDetector, Region},
    scan::{scan_video, ScanOptions},
};

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn banner(w: u32, h: u32) -> GrayImage {
    // A chevron plus a bar: distinctive, not flat.
    GrayImage::from_fn(w, h, |x, y| {
        let cx = w as i32 / 2;
        let chevron = ((x as i32 - cx).abs() - (y as i32)).abs() < 4 && y < h * 2 / 3;
        let bar = y > h * 3 / 4 && y < h * 3 / 4 + 6 && x > w / 6 && x < w * 5 / 6;
        Luma([if chevron || bar { 235 } else { 25 }])
    })
}

#[test]
fn finds_banner_appearances_in_a_real_video() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let tpl_path = dir.path().join("templates");
    std::fs::create_dir_all(&tpl_path).unwrap();
    let tpl = banner(120, 60);
    tpl.save(tpl_path.join("kill.png")).unwrap();
    let video: PathBuf = dir.path().join("match.mp4");

    // 1280x720, 12 s, noisy grey background; banner at (580, 520) during 3.0–4.5 s and 8.0–9.5 s.
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "color=c=0x404040:s=1280x720:r=30:d=12"])
        .arg("-i")
        .arg(tpl_path.join("kill.png"))
        .args([
            "-filter_complex",
            "[0:v]noise=alls=25:allf=t[bg];[bg][1:v]overlay=580:520:enable='between(t,3,4.5)+between(t,8,9.5)',format=yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
        ])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());

    let def = DetectorDef {
        id: "kill".into(),
        label: "Kill".into(),
        event: "kill".into(),
        // Template box plus a margin, as Detector Studio saves it.
        region: Region {
            x: 540.0 / 1280.0,
            y: 490.0 / 720.0,
            w: 200.0 / 1280.0,
            h: 120.0 / 720.0,
        },
        template_box: None,
        templates: vec!["templates/kill.png".into()],
        template_frame_height: 720,
        method: DetectMethod::Ncc,
        threshold: 0.7,
        cooldown_ms: 1500,
        min_consecutive: 2,
        sample_fps: 6.0,
        scales: vec![],
        rearm_drop: 0.0,
        enabled: true,
        notes: None,
    };
    let det = LoadedDetector {
        def,
        base_dir: dir.path().to_path_buf(),
        user: true,
    };
    let ff = Ffmpeg {
        ffmpeg: "ffmpeg".into(),
        ffprobe: "ffprobe".into(),
    };
    // Scan at 540p to also exercise template rescaling (template was cut at 720p).
    let res = scan_video(
        &ff,
        &video,
        &[det],
        ScanOptions {
            analysis_height: 540,
            keep_traces: true,
            ..Default::default()
        },
    )
    .unwrap();

    let times: Vec<u64> = res.events.iter().map(|e| e.t_ms).collect();
    assert_eq!(times.len(), 2, "events: {times:?}");
    assert!((2800..=3400).contains(&times[0]), "first at {}", times[0]);
    assert!((7800..=8400).contains(&times[1]), "second at {}", times[1]);
    let trace = &res.traces[0].samples;
    let peak_on = trace
        .iter()
        .filter(|(t, _)| (3200..4300).contains(t))
        .map(|s| s.1)
        .fold(0.0, f32::max);
    let peak_off = trace
        .iter()
        .filter(|(t, _)| (5000..7500).contains(t))
        .map(|s| s.1)
        .fold(-1.0, f32::max);
    assert!(peak_on > 0.85, "on-screen score {peak_on}");
    assert!(peak_off < 0.5, "off-screen score {peak_off}");
}
