//! The recorder's pre-roll buffer: while a game is open the output writes short rotating
//! segments into a buffer folder next to the recordings. When a match starts, the segment in
//! progress becomes the match file (so it begins up to the pre-roll before the start request);
//! when it ends, the segment is renamed to the match path.
//!
//! A segment that is a recording gets a `<segment>.target` marker holding the final path. If
//! the recorder or the app dies mid-match, [`recover`] moves such segments to where they
//! belong. Segments without a marker are disposable.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Name of the buffer folder inside the recordings folder.
pub const BUFFER_DIR_NAME: &str = ".recap-buffer";
const SEGMENT_PREFIX: &str = "seg-";
const MARKER_EXT: &str = "target";

/// File stem of segment `n` of buffer run `run` (OBS appends `.mp4`).
pub fn segment_stem(run: u64, n: u64) -> String {
    format!("{SEGMENT_PREFIX}{run:x}-{n:06}")
}

/// Full path of a segment.
pub fn segment_path(dir: &Path, run: u64, n: u64) -> PathBuf {
    dir.join(format!("{}.mp4", segment_stem(run, n)))
}

pub fn is_segment(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(SEGMENT_PREFIX))
}

/// `<segment>.mp4.target`
pub fn marker_path(segment: &Path) -> PathBuf {
    let mut s = segment.as_os_str().to_owned();
    s.push(".");
    s.push(MARKER_EXT);
    PathBuf::from(s)
}

/// Mark `segment` as the recording for `target`.
pub fn write_marker(segment: &Path, target: &Path) -> std::io::Result<()> {
    fs::write(marker_path(segment), target.to_string_lossy().as_bytes())
}

pub fn remove_marker(segment: &Path) {
    let _ = fs::remove_file(marker_path(segment));
}

/// Move a finished segment to its final path: rename, or copy + delete across drives.
pub fn move_segment(segment: &Path, target: &Path) -> std::io::Result<()> {
    if let Some(p) = target.parent() {
        fs::create_dir_all(p)?;
    }
    match fs::rename(segment, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(segment, target)?;
            fs::remove_file(segment)
        }
    }
}

/// Delete segments that are not recordings (no marker). Returns how many were deleted.
/// Call only when no output is writing into `dir`.
pub fn clean(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        if is_segment(&p) && !marker_path(&p).exists() && fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

/// Move every marked segment to its target (an interrupted recording). Returns the targets
/// that now exist. Call only when no output is writing into `dir`.
pub fn recover(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return vec![] };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let marker = e.path();
        if marker.extension().and_then(|x| x.to_str()) != Some(MARKER_EXT) {
            continue;
        }
        let segment = marker.with_extension("");
        let target = fs::read_to_string(&marker).ok().map(|t| PathBuf::from(t.trim()));
        match target {
            Some(t) if !t.as_os_str().is_empty() && segment.exists() && !t.exists() => {
                if move_segment(&segment, &t).is_ok() {
                    let _ = fs::remove_file(&marker);
                    out.push(t);
                }
            }
            // Already moved (or nothing to move): drop the marker; a leftover segment becomes
            // disposable.
            _ => {
                let _ = fs::remove_file(&marker);
            }
        }
    }
    out
}

/// What to do with the buffer when a recording starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPlan {
    /// The segment in progress becomes the recording; it began `preroll_ms` ago.
    KeepCurrent { preroll_ms: u64 },
    /// Start a fresh segment first (no pre-roll wanted, or the segment is far too old because
    /// rotation stalled).
    SplitFirst,
}

/// A segment may run this much past the pre-roll before it is considered stale (rotation
/// waits for a keyframe, and the buffer is polled).
pub const ROTATION_SLACK_MS: u64 = 2_500;

pub fn plan_start(segment_age_ms: u64, preroll_ms: u64) -> StartPlan {
    if preroll_ms == 0 || segment_age_ms > preroll_ms + ROTATION_SLACK_MS {
        StartPlan::SplitFirst
    } else {
        StartPlan::KeepCurrent {
            preroll_ms: segment_age_ms,
        }
    }
}

/// Rotate the warm segment once it covers the whole pre-roll.
pub fn should_rotate(segment_age_ms: u64, preroll_ms: u64) -> bool {
    preroll_ms > 0 && segment_age_ms >= preroll_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        assert_eq!(plan_start(4_000, 10_000), StartPlan::KeepCurrent { preroll_ms: 4_000 });
        assert_eq!(plan_start(11_000, 10_000), StartPlan::KeepCurrent { preroll_ms: 11_000 });
        assert_eq!(plan_start(30_000, 10_000), StartPlan::SplitFirst);
        assert_eq!(plan_start(10, 0), StartPlan::SplitFirst);
        assert!(should_rotate(10_000, 10_000));
        assert!(!should_rotate(9_999, 10_000));
        assert!(!should_rotate(99_999, 0));
    }

    #[test]
    fn names() {
        let d = Path::new("/r/.recap-buffer");
        let p = segment_path(d, 0xabc, 7);
        assert_eq!(p, Path::new("/r/.recap-buffer/seg-abc-000007.mp4"));
        assert!(is_segment(&p));
        assert!(!is_segment(Path::new("/r/.recap-buffer/match.mp4")));
        assert_eq!(marker_path(&p), Path::new("/r/.recap-buffer/seg-abc-000007.mp4.target"));
    }

    #[test]
    fn clean_and_recover() {
        let d = tempfile::tempdir().unwrap();
        let buf = d.path().join(BUFFER_DIR_NAME);
        fs::create_dir_all(&buf).unwrap();
        let junk = segment_path(&buf, 1, 1);
        let rec = segment_path(&buf, 1, 2);
        fs::write(&junk, b"menu").unwrap();
        fs::write(&rec, b"match").unwrap();
        let target = d.path().join("Game").join("2026-10-24_20-00-00.mp4");
        write_marker(&rec, &target).unwrap();
        fs::write(buf.join("notes.txt"), b"keep").unwrap();

        assert_eq!(clean(&buf), 1);
        assert!(!junk.exists() && rec.exists() && buf.join("notes.txt").exists());

        assert_eq!(recover(&buf), vec![target.clone()]);
        assert_eq!(fs::read(&target).unwrap(), b"match");
        assert!(!rec.exists() && !marker_path(&rec).exists());
        // Nothing left to do.
        assert!(recover(&buf).is_empty());

        // A marker whose target already exists is dropped; its segment becomes disposable.
        let again = segment_path(&buf, 1, 3);
        fs::write(&again, b"dup").unwrap();
        write_marker(&again, &target).unwrap();
        assert!(recover(&buf).is_empty());
        assert!(!marker_path(&again).exists());
        assert_eq!(clean(&buf), 1);
    }
}
