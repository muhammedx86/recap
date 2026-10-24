//! Render the sample game's whole timeline into a real H.264 video and check Recap's
//! scanner finds every banner exactly once, at the right time, with nothing extra.
//! Skipped when ffmpeg/ffprobe are not installed.

use std::{
    io::Write,
    process::{Command, Stdio},
};

use recap_core::{
    ffmpeg::Ffmpeg,
    games::load_games,
    scan::{scan_video, ScanOptions},
    segments::{plan_match_segments, SegmentOptions},
};
use recap_sample_game::{export_fixture, new_frame, render_frame, timeline, EXIT_AT_S, HEIGHT, WIDTH};

/// Recorded at the detectors' sample rate to keep the debug-mode test quick.
const FPS: f64 = 6.0;

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn scanner_finds_every_banner_once() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    export_fixture(tmp.path()).unwrap();
    let video = tmp.path().join("session.mp4");

    // Stream raw frames straight into ffmpeg (no PNG round-trip).
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
            "-s",
        ])
        .arg(format!("{WIDTH}x{HEIGHT}"))
        .args([
            "-r",
            &FPS.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&video)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        let mut buf = new_frame();
        let mut bytes = vec![0u8; WIDTH * HEIGHT * 4];
        let n = (EXIT_AT_S * FPS).round() as usize;
        for i in 0..n {
            render_frame(i as f64 / FPS, &mut buf);
            for (px, out) in buf.iter().zip(bytes.chunks_exact_mut(4)) {
                // 0RGB u32 -> BGRA bytes (little endian).
                out.copy_from_slice(&(px | 0xff00_0000).to_le_bytes());
            }
            stdin.write_all(&bytes).unwrap();
        }
    }
    assert!(child.wait().unwrap().success());

    let games = load_games(&[tmp.path().to_path_buf()]);
    let dets: Vec<_> = games[0].ready_detectors().cloned().collect();
    assert_eq!(dets.len(), 6);
    let ff = Ffmpeg {
        ffmpeg: "ffmpeg".into(),
        ffprobe: "ffprobe".into(),
    };
    let res = scan_video(
        &ff,
        &video,
        &dets,
        ScanOptions {
            analysis_height: HEIGHT as u32,
            keep_traces: true,
            ..Default::default()
        },
    )
    .unwrap();

    let found: Vec<(String, u64)> = res.events.iter().map(|e| (e.kind.clone(), e.t_ms)).collect();
    let expected = timeline();
    let mut unmatched: Vec<(String, u64)> = found.clone();
    for cue in &expected {
        let want = (cue.t_s * 1000.0) as u64;
        let pos = unmatched
            .iter()
            .position(|(k, t)| k == cue.kind.kind() && t.abs_diff(want) <= 500)
            .unwrap_or_else(|| {
                let peaks: Vec<_> = res
                    .traces
                    .iter()
                    .map(|tr| {
                        let p = tr
                            .samples
                            .iter()
                            .filter(|(t, _)| (want..want + (cue.duration_s * 1000.0) as u64).contains(t))
                            .map(|s| s.1)
                            .fold(-1.0f32, f32::max);
                        (tr.detector.clone(), p)
                    })
                    .collect();
                panic!(
                    "{} at {want} ms not detected. found: {found:?}\npeak scores during it: {peaks:?}",
                    cue.kind.kind()
                )
            });
        unmatched.remove(pos);
    }
    assert!(unmatched.is_empty(), "extra events: {unmatched:?}\nall: {found:?}");

    let counts = |k: &str| found.iter().filter(|(kind, _)| kind == k).count();
    assert_eq!(
        (
            counts("kill"),
            counts("round_start"),
            counts("round_win"),
            counts("round_loss"),
            counts("match_start"),
            counts("match_end")
        ),
        (4, 3, 2, 1, 2, 2)
    );

    // The session splits into the two matches.
    let duration = (EXIT_AT_S * 1000.0) as u64;
    let segs = plan_match_segments(
        &res.events,
        duration,
        &SegmentOptions {
            min_len_ms: 5000,
            ..Default::default()
        },
    );
    assert_eq!(segs.len(), 2, "{segs:?}");
    assert!(segs[0].start_ms <= 10_000 && segs[0].end_ms >= 31_000, "{segs:?}");
    assert!(segs[1].start_ms <= 45_000 && segs[1].end_ms >= 58_000, "{segs:?}");

    // Each detector stays well below threshold when its banner is absent (no near-misses).
    for tr in &res.traces {
        let kind = &dets.iter().find(|d| d.def.id == tr.detector).unwrap().def.event;
        let shown = |t: u64| {
            expected
                .iter()
                .any(|c| c.kind.kind() == kind && (t as f64) >= c.t_s * 1000.0 - 200.0 && (t as f64) <= c.end_s() * 1000.0 + 200.0)
        };
        let worst_off = tr.samples.iter().filter(|(t, _)| !shown(*t)).map(|s| s.1).fold(-1.0f32, f32::max);
        let weakest_on = expected
            .iter()
            .filter(|c| c.kind.kind() == kind)
            .map(|c| {
                tr.samples
                    .iter()
                    .filter(|(t, _)| (*t as f64) >= c.t_s * 1000.0 + 300.0 && (*t as f64) <= c.end_s() * 1000.0 - 300.0)
                    .map(|s| s.1)
                    .fold(-1.0f32, f32::max)
            })
            .fold(1.0f32, f32::min);
        eprintln!("{:<12} shown peak >= {weakest_on:.3}, hidden max {worst_off:.3}", tr.detector);
        assert!(weakest_on > 0.9, "{} only reaches {weakest_on} while shown", tr.detector);
        assert!(worst_off < 0.6, "{} scores {worst_off} while hidden", tr.detector);
    }
}
