//! Cheap previews for the library and the match page: the cut points a clip will really use, an
//! audio waveform, and a strip of frames for hover scrubbing.

use std::{io::Read, path::Path, process::Stdio};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::ffmpeg::{format_secs, Ffmpeg};

/// Where a clip of `[start, end)` really starts and ends once cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CutPoints {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Frame-accurate (re-encoded) cut: the points are exactly what was asked for.
    pub exact: bool,
}

/// Keyframes are searched this far before the requested start (GOPs are a few seconds at most).
const KEYFRAME_LOOKBACK_MS: u64 = 15_000;

/// Container start time in seconds: ffprobe reports absolute timestamps, ffmpeg's `-ss` is
/// relative to this.
fn start_offset_secs(ff: &Ffmpeg, video: &Path) -> f64 {
    ff.ffprobe_command()
        .args(["-v", "error", "-show_entries", "format=start_time", "-of", "csv=p=0"])
        .arg(video)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Parse `ffprobe -show_entries packet=pts_time,flags -of csv=p=0` lines into keyframe times
/// (ms, relative to `offset_secs`).
pub fn parse_keyframes(csv: &str, offset_secs: f64) -> Vec<u64> {
    let mut out: Vec<u64> = csv
        .lines()
        .filter_map(|line| {
            let mut parts = line.split(',');
            let t: f64 = parts.next()?.trim().parse().ok()?;
            let flags = parts.next_back()?;
            flags.contains('K').then(|| ((t - offset_secs).max(0.0) * 1000.0).round() as u64)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Video keyframes between `from_ms` and `to_ms` (relative to the start of the file).
pub fn keyframes_between(ff: &Ffmpeg, video: &Path, from_ms: u64, to_ms: u64) -> Result<Vec<u64>> {
    let offset = start_offset_secs(ff, video);
    let a = offset + from_ms as f64 / 1000.0;
    let b = offset + to_ms.max(from_ms + 1) as f64 / 1000.0;
    let out = ff
        .ffprobe_command()
        .args(["-v", "error", "-select_streams", "v:0", "-read_intervals"])
        .arg(format!("{a:.3}%{b:.3}"))
        .args(["-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
        .arg(video)
        .stdin(Stdio::null())
        .output()
        .context("running ffprobe")?;
    if !out.status.success() {
        bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(parse_keyframes(&String::from_utf8_lossy(&out.stdout), offset))
}

/// A stream-copy cut starts at the last keyframe at or before the requested start.
pub fn stream_copy_start(keyframes: &[u64], start_ms: u64) -> u64 {
    keyframes.iter().rev().find(|&&k| k <= start_ms + 1).copied().unwrap_or(start_ms)
}

/// The range a clip of `[start_ms, end_ms)` really covers. Stream copies (`precise = false`)
/// start on the keyframe before `start_ms`; re-encoded cuts are exact.
pub fn cut_points(ff: &Ffmpeg, video: &Path, start_ms: u64, end_ms: u64, precise: bool) -> Result<CutPoints> {
    if precise {
        return Ok(CutPoints {
            start_ms,
            end_ms,
            exact: true,
        });
    }
    let kf = keyframes_between(ff, video, start_ms.saturating_sub(KEYFRAME_LOOKBACK_MS), start_ms + 1)?;
    Ok(CutPoints {
        start_ms: stream_copy_start(&kf, start_ms),
        end_ms,
        exact: false,
    })
}

/// Peaks of the first audio track in `buckets` slots, 0..255 (0 = silence). Empty when the
/// recording has no audio.
pub fn waveform(ff: &Ffmpeg, video: &Path, duration_ms: u64, buckets: usize) -> Result<Vec<u8>> {
    const RATE: u64 = 8000;
    if buckets == 0 || duration_ms == 0 || !ff.has_audio(video) {
        return Ok(vec![]);
    }
    let mut child = ff
        .command()
        .arg("-i")
        .arg(video)
        .args(["-map", "0:a:0", "-vn", "-ac", "1", "-ar"])
        .arg(RATE.to_string())
        .args(["-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running ffmpeg")?;
    let mut stdout = child.stdout.take().context("no ffmpeg output")?;
    let total = (duration_ms * RATE / 1000).max(1);
    let mut peaks = vec![0u16; buckets];
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry: Option<u8> = None;
    let mut index: u64 = 0;
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut bytes = &buf[..n];
        if let Some(lo) = carry.take() {
            let s = i16::from_le_bytes([lo, bytes[0]]);
            add_peak(&mut peaks, index, total, s);
            index += 1;
            bytes = &bytes[1..];
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        for c in pairs {
            add_peak(&mut peaks, index, total, i16::from_le_bytes(*c));
            index += 1;
        }
        if let [lo] = rest {
            carry = Some(*lo);
        }
    }
    let _ = child.wait();
    Ok(normalize_peaks(&peaks))
}

fn add_peak(peaks: &mut [u16], index: u64, total: u64, s: i16) {
    let b = ((index * peaks.len() as u64) / total).min(peaks.len() as u64 - 1) as usize;
    let v = s.unsigned_abs();
    if v > peaks[b] {
        peaks[b] = v;
    }
}

/// Scale peaks to 0..255 against the loudest one, with a square-root curve so quiet passages
/// still show.
pub fn normalize_peaks(peaks: &[u16]) -> Vec<u8> {
    let max = peaks.iter().copied().max().unwrap_or(0).max(1) as f32;
    peaks.iter().map(|&p| ((p as f32 / max).sqrt() * 255.0).round() as u8).collect()
}

/// Times of `frames` evenly spaced preview frames (the middle of each slot).
pub fn strip_times(duration_ms: u64, frames: u32) -> Vec<u64> {
    (0..frames)
        .map(|i| ((i as f64 + 0.5) / frames as f64 * duration_ms as f64) as u64)
        .collect()
}

/// One JPEG of `frames` evenly spaced frames side by side, each `width` px wide (16:9 slots),
/// made with a single ffmpeg call (one fast seek per frame).
pub fn scrub_strip(ff: &Ffmpeg, video: &Path, duration_ms: u64, frames: u32, width: u32, out: &Path) -> Result<()> {
    if frames == 0 || duration_ms == 0 {
        bail!("nothing to preview");
    }
    let w = width.max(16) & !1;
    let h = ((w * 9 / 16).max(2)) & !1;
    let mut cmd = ff.command();
    cmd.arg("-y");
    for t in strip_times(duration_ms, frames) {
        cmd.arg("-ss").arg(format_secs(t)).arg("-i").arg(video);
    }
    let mut graph = String::new();
    for i in 0..frames {
        graph.push_str(&format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1[f{i}];"
        ));
    }
    for i in 0..frames {
        graph.push_str(&format!("[f{i}]"));
    }
    graph.push_str(&format!("hstack=inputs={frames}[out]"));
    if frames == 1 {
        graph = format!("[0:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1[out]");
    }
    let status = cmd
        .arg("-filter_complex")
        .arg(graph)
        .args(["-map", "[out]", "-frames:v", "1", "-q:v", "5"])
        .arg(out)
        .stdin(Stdio::null())
        .status()
        .context("running ffmpeg")?;
    if !status.success() || !out.exists() {
        bail!("could not make the preview strip for {}", video.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_parsing_and_snapping() {
        let csv = "10.000000,K__\n10.033000,___\n12.000000,K_\n12.033,__\nN/A,K\n";
        let kf = parse_keyframes(csv, 0.0);
        assert_eq!(kf, vec![10_000, 12_000]);
        assert_eq!(stream_copy_start(&kf, 11_500), 10_000);
        assert_eq!(stream_copy_start(&kf, 12_000), 12_000);
        assert_eq!(stream_copy_start(&kf, 9_000), 9_000);
        // Container start offset (OBS files often start a little after 0).
        assert_eq!(parse_keyframes("1.400000,K__\n", 1.4), vec![0]);
    }

    #[test]
    fn peaks_and_strip_times() {
        assert_eq!(normalize_peaks(&[0, 100, 400]), vec![0, 128, 255]);
        assert_eq!(normalize_peaks(&[0, 0]), vec![0, 0]);
        assert_eq!(strip_times(10_000, 4), vec![1250, 3750, 6250, 8750]);
    }

    fn ffmpeg_available() -> Option<Ffmpeg> {
        let ff = Ffmpeg::locate(&[]);
        ff.command()
            .arg("-version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| ff)
    }

    #[test]
    fn ffmpeg_previews() {
        let Some(ff) = ffmpeg_available() else {
            eprintln!("ffmpeg not found; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.mp4");
        // 6 s test video, keyframe every 2 s, with a sine tone in the second half.
        let ok = ff
            .command()
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=30:duration=6",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc='if(gt(t,3),0.8*sin(2*PI*440*t),0)':s=48000:d=6",
                "-c:v",
                "libx264",
                "-g",
                "60",
                "-keyint_min",
                "60",
                "-sc_threshold",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);

        let kf = keyframes_between(&ff, &src, 0, 6000).unwrap();
        assert_eq!(kf, vec![0, 2000, 4000], "keyframes {kf:?}");
        let cp = cut_points(&ff, &src, 3_100, 5_000, false).unwrap();
        assert_eq!(
            cp,
            CutPoints {
                start_ms: 2000,
                end_ms: 5000,
                exact: false
            }
        );
        assert!(cut_points(&ff, &src, 3_100, 5_000, true).unwrap().exact);
        // The stream-copy clip really covers the snapped range.
        let clip = dir.path().join("clip.mp4");
        crate::clips::cut_clip(&ff, &src, 3_100, 5_000, &clip, false).unwrap();
        let got = ff.probe(&clip).unwrap().duration_ms;
        assert!(
            (2_850..=3_200).contains(&got),
            "clip covers {got} ms, expected ~3000 (2.0 s to 5.0 s)"
        );

        let wave = waveform(&ff, &src, 6000, 12).unwrap();
        assert_eq!(wave.len(), 12);
        assert!(wave[..5].iter().all(|&v| v < 40), "silent first half {wave:?}");
        assert!(wave[7..].iter().all(|&v| v > 200), "loud second half {wave:?}");

        let strip = dir.path().join("strip.jpg");
        scrub_strip(&ff, &src, 6000, 4, 96, &strip).unwrap();
        let img = image::open(&strip).unwrap();
        assert_eq!((img.width(), img.height()), (96 * 4, 54));
    }
}
