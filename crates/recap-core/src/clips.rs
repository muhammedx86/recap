//! Clip planning (which windows to cut) and cutting/joining with ffmpeg.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::{
    events::GameEvent,
    ffmpeg::{format_secs, Ffmpeg},
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipWindow {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Time of the first event that caused this clip.
    pub anchor_ms: u64,
    /// Distinct kinds of all events inside the window, in time order.
    pub event_kinds: Vec<String>,
}

/// Plan auto-clips: one window per event whose kind is in `kinds`, padded by
/// `before_ms`/`after_ms`, clamped to the recording (when `duration_ms > 0`),
/// and merged when windows overlap or are closer than `merge_gap_ms`.
pub fn plan_auto_clips(
    events: &[GameEvent],
    kinds: &[String],
    before_ms: u64,
    after_ms: u64,
    merge_gap_ms: u64,
    duration_ms: u64,
) -> Vec<ClipWindow> {
    let mut triggers: Vec<&GameEvent> = events.iter().filter(|e| kinds.iter().any(|k| k == &e.kind)).collect();
    triggers.sort_by_key(|e| e.t_ms);

    let mut windows: Vec<ClipWindow> = Vec::new();
    for e in triggers {
        let mut start = e.t_ms.saturating_sub(before_ms);
        let mut end = e.t_ms.saturating_add(after_ms);
        if duration_ms > 0 {
            end = end.min(duration_ms);
            start = start.min(end);
        }
        if end <= start {
            continue;
        }
        match windows.last_mut() {
            Some(last) if start <= last.end_ms.saturating_add(merge_gap_ms) => {
                last.end_ms = last.end_ms.max(end);
            }
            _ => windows.push(ClipWindow {
                start_ms: start,
                end_ms: end,
                anchor_ms: e.t_ms,
                event_kinds: vec![],
            }),
        }
    }
    for w in &mut windows {
        w.event_kinds = kinds_in_window(events, w.start_ms, w.end_ms);
    }
    windows
}

/// Distinct kinds of events with `start_ms <= t <= end_ms`, in time order.
pub fn kinds_in_window(events: &[GameEvent], start_ms: u64, end_ms: u64) -> Vec<String> {
    let mut inside: Vec<&GameEvent> = events.iter().filter(|e| e.t_ms >= start_ms && e.t_ms <= end_ms).collect();
    inside.sort_by_key(|e| e.t_ms);
    let mut out: Vec<String> = Vec::new();
    for e in inside {
        if !out.contains(&e.kind) {
            out.push(e.kind.clone());
        }
    }
    out
}

fn temp_path(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip.mp4".into());
    let ext = out
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp4".into());
    out.with_file_name(format!(".{name}.part.{ext}"))
}

fn run(mut cmd: std::process::Command, what: &str) -> Result<()> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running ffmpeg ({what})"))?;
    if !out.status.success() {
        bail!("ffmpeg {what} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn finish(tmp: &Path, out: &Path) -> Result<()> {
    let ok = fs::metadata(tmp).map(|m| m.len() > 0).unwrap_or(false);
    if !ok {
        let _ = fs::remove_file(tmp);
        bail!("ffmpeg produced no output for {}", out.display());
    }
    if out.exists() {
        fs::remove_file(out)?;
    }
    fs::rename(tmp, out)?;
    Ok(())
}

/// Cut `[start_ms, end_ms)` of `src` into `out`.
///
/// `precise = false`: stream copy (instant; the start snaps to the previous keyframe).
/// `precise = true`: re-encode with x264/AAC for frame-accurate cuts.
pub fn cut_clip(ff: &Ffmpeg, src: &Path, start_ms: u64, end_ms: u64, out: &Path, precise: bool) -> Result<()> {
    if end_ms <= start_ms {
        bail!("clip end must be after start");
    }
    if let Some(p) = out.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = temp_path(out);
    let mut cmd = ff.command();
    cmd.arg("-y")
        .arg("-ss")
        .arg(format_secs(start_ms))
        .arg("-i")
        .arg(src)
        .arg("-t")
        .arg(format_secs(end_ms - start_ms));
    cmd.args(["-map", "0:v:0", "-map", "0:a?"]);
    if precise {
        cmd.args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k",
        ]);
    } else {
        cmd.args(["-c", "copy", "-avoid_negative_ts", "make_zero"]);
    }
    cmd.args(["-movflags", "+faststart", "-f", "mp4"]).arg(&tmp);
    if let Err(e) = run(cmd, "cut") {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    finish(&tmp, out)
}

/// Escape a path for an ffmpeg concat list (`file '...'`).
fn concat_entry(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/").replace('\'', "'\\''");
    format!("file '{s}'\n")
}

/// Join clips into one video. Tries a lossless concat first and falls back to re-encoding
/// (needed when clips differ in resolution, codec or audio layout).
pub fn concat(ff: &Ffmpeg, inputs: &[PathBuf], out: &Path) -> Result<()> {
    if inputs.is_empty() {
        bail!("nothing to join");
    }
    if let Some(p) = out.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = temp_path(out);
    let list = out.with_file_name(format!(
        ".{}.concat.txt",
        out.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "montage".into())
    ));
    let list_body: String = inputs
        .iter()
        .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .map(|p| concat_entry(&p))
        .collect();
    fs::write(&list, list_body)?;

    let mut copy = ff.command();
    copy.args(["-y", "-f", "concat", "-safe", "0", "-i"]).arg(&list);
    copy.args([
        "-map",
        "0:v:0",
        "-map",
        "0:a?",
        "-c",
        "copy",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ])
    .arg(&tmp);
    let copied = run(copy, "concat");
    let _ = fs::remove_file(&list);
    if copied.is_ok() && finish(&tmp, out).is_ok() {
        return Ok(());
    }
    let _ = fs::remove_file(&tmp);
    log::info!("lossless concat failed ({:?}); re-encoding", copied.err());

    // Fallback: decode everything and join with the concat filter. Inputs without audio
    // get a silent track so the filter graph is uniform.
    let first = ff.probe(&inputs[0]).ok();
    let (w, h) = first.map(|i| (i.width, i.height)).unwrap_or((1920, 1080));
    let mut cmd = ff.command();
    cmd.arg("-y");
    for p in inputs {
        cmd.arg("-i").arg(p);
    }
    let mut filter = String::new();
    for (i, input) in inputs.iter().enumerate() {
        filter.push_str(&format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=60,format=yuv420p[v{i}];"
        ));
        if ff.has_audio(input) {
            filter.push_str(&format!("[{i}:a:0]aformat=sample_rates=48000:channel_layouts=stereo[a{i}];"));
        } else {
            // Silent track of the same length so every segment has audio.
            filter.push_str(&format!(
                "aevalsrc=0:c=stereo:s=48000,atrim=duration={}[a{i}];",
                duration_secs(ff, input)
            ));
        }
    }
    for i in 0..inputs.len() {
        filter.push_str(&format!("[v{i}][a{i}]"));
    }
    filter.push_str(&format!("concat=n={}:v=1:a=1[v][a]", inputs.len()));
    cmd.arg("-filter_complex").arg(filter);
    cmd.args([
        "-map", "[v]", "-map", "[a]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-c:a", "aac", "-b:a", "160k",
    ]);
    cmd.args(["-movflags", "+faststart", "-f", "mp4"]).arg(&tmp);
    if let Err(e) = run(cmd, "montage re-encode") {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    finish(&tmp, out)
}

fn duration_secs(ff: &Ffmpeg, p: &Path) -> String {
    ff.probe(p).map(|i| format_secs(i.duration_ms)).unwrap_or_else(|_| "1".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{kind, EventSource};

    fn ev(k: &str, t: u64) -> GameEvent {
        GameEvent::new(k, t, EventSource::Manual)
    }

    #[test]
    fn plans_merge_and_clamp() {
        let events = vec![
            ev(kind::KILL, 1_000),
            ev(kind::DEATH, 9_000),
            ev(kind::KILL, 10_000),
            ev(kind::KILL, 14_000), // overlaps the previous window -> merged
            ev(kind::KILL, 40_000),
            ev(kind::BOOKMARK, 59_500),
        ];
        let kinds = vec![kind::KILL.to_string(), kind::BOOKMARK.to_string()];
        let w = plan_auto_clips(&events, &kinds, 5_000, 3_000, 500, 60_000);
        assert_eq!(w.len(), 4);
        assert_eq!((w[0].start_ms, w[0].end_ms, w[0].anchor_ms), (0, 4_000, 1_000), "clamped at 0");
        assert_eq!((w[1].start_ms, w[1].end_ms, w[1].anchor_ms), (5_000, 17_000, 10_000));
        assert_eq!(w[1].event_kinds, vec!["death", "kill"]);
        assert_eq!((w[2].start_ms, w[2].end_ms), (35_000, 43_000));
        assert_eq!((w[3].start_ms, w[3].end_ms), (54_500, 60_000), "clamped at duration");
        assert_eq!(w[3].event_kinds, vec!["bookmark"]);
    }

    #[test]
    fn merges_within_gap() {
        let events = vec![ev(kind::KILL, 10_000), ev(kind::KILL, 19_500)];
        let kinds = vec![kind::KILL.to_string()];
        // Windows [5s,13s] and [14.5s,22.5s] are 1.5s apart.
        assert_eq!(plan_auto_clips(&events, &kinds, 5_000, 3_000, 1_000, 0).len(), 2);
        let merged = plan_auto_clips(&events, &kinds, 5_000, 3_000, 2_000, 0);
        assert_eq!((merged.len(), merged[0].start_ms, merged[0].end_ms), (1, 5_000, 22_500));
    }

    #[test]
    fn plans_nothing_without_matching_kinds() {
        assert!(plan_auto_clips(&[ev(kind::DEATH, 1000)], &["kill".into()], 1000, 1000, 0, 0).is_empty());
    }

    #[test]
    fn concat_list_escapes_quotes() {
        assert_eq!(concat_entry(Path::new("C:\\clips\\it's.mp4")), "file 'C:/clips/it'\\''s.mp4'\n");
    }

    fn ffmpeg_available() -> Option<Ffmpeg> {
        let ff = Ffmpeg::locate(&[]);
        let ok = std::process::Command::new(&ff.ffmpeg)
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            && std::process::Command::new(&ff.ffprobe)
                .arg("-version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
        ok.then_some(ff)
    }

    #[test]
    fn ffmpeg_cut_and_concat() {
        let Some(ff) = ffmpeg_available() else {
            eprintln!("ffmpeg not found; skipping");
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src.mp4");
        let st = ff
            .command()
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
            ])
            .args([
                "-t",
                "10",
                "-c:v",
                "libx264",
                "-g",
                "30",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&src)
            .status()
            .unwrap();
        assert!(st.success());

        let a = d.path().join("clips/a.mp4");
        let b = d.path().join("clips/b.mp4");
        cut_clip(&ff, &src, 1_000, 3_000, &a, false).unwrap();
        cut_clip(&ff, &src, 5_500, 8_000, &b, true).unwrap();
        let da = ff.probe(&a).unwrap().duration_ms;
        let db = ff.probe(&b).unwrap().duration_ms;
        assert!((1_800..=2_300).contains(&da), "copy clip {da}ms");
        assert!((2_400..=2_600).contains(&db), "precise clip {db}ms");
        assert!(!temp_path(&a).exists());

        let m = d.path().join("montage.mp4");
        concat(&ff, &[a.clone(), b.clone()], &m).unwrap();
        let dm = ff.probe(&m).unwrap().duration_ms;
        assert!((4_100..=5_000).contains(&dm), "montage {dm}ms");

        assert!(cut_clip(&ff, &src, 3_000, 3_000, &a, false).is_err());
        assert!(concat(&ff, &[], &m).is_err());
    }
}
