//! Detection while the match is being recorded.
//!
//! The recorder hands [`LiveScanner`] a small grey copy of the picture several times a second
//! (scaled on the GPU, so it costs the game almost nothing) and the scanner runs the game's
//! detectors on it exactly as the post-match scan would: same regions, references, thresholds
//! and triggers. When a recording stops, [`LiveScanner::take`] returns its events, so the
//! markers are ready the moment the match ends instead of after decoding the whole video.
//!
//! Times are wall-clock milliseconds (unix); `take` turns them into times from the start of the
//! recording.

use image::GrayImage;
use serde::{Deserialize, Serialize};

use crate::{
    events::{EventSource, GameEvent},
    ffmpeg::ContentRect,
    games::LoadedDetector,
    scan::{apply_blockers, Blocker},
    vision::{FrameRef, PreparedDetector, Trigger, ValueTrigger},
};

/// What live detection found in one recording.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveScan {
    pub events: Vec<GameEvent>,
    /// Detector ids that ran for the whole recording (the post-match scan can skip them).
    pub detectors: Vec<String>,
    /// Frames looked at, and the share of the recording they covered (0..1).
    pub frames: u64,
    pub coverage: f32,
}

/// A detector as sent to the recorder: its definition and the folder its references are in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDetector {
    pub def: crate::games::DetectorDef,
    pub base_dir: String,
}

/// Live detection settings for a recording session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveConfig {
    pub detectors: Vec<LiveDetector>,
    /// Height of the frames the detectors look at (the post-match scan's analysis height).
    pub analysis_height: u32,
}

impl LiveConfig {
    /// Samples per second the detectors need (the fastest one).
    pub fn sample_fps(&self) -> f32 {
        self.detectors
            .iter()
            .filter(|d| live_capable(&d.def))
            .map(|d| d.def.sample_fps)
            .fold(0.5, f32::max)
            .min(10.0)
    }
}

/// Live frames are grey: detectors that filter by colour stay with the post-match scan.
pub fn live_capable(def: &crate::games::DetectorDef) -> bool {
    def.is_ready() && def.color_filter.is_none()
}

struct Slot {
    det: PreparedDetector,
    trig: Trigger,
    value: Option<ValueTrigger>,
    period_ms: u64,
    last_sample: Option<u64>,
    spans: Vec<(u64, u64)>,
}

/// Frames used to find black bars around the game picture.
const BAR_FRAMES: u32 = 24;
/// Look for bars again this often (the game may change resolution or window mode).
const BAR_RECHECK_MS: u64 = 30_000;
/// Brightest a bar pixel may be (same as the post-match scan's cropdetect limit).
const BAR_LIMIT: u8 = 24;
/// A gap between frames longer than this means frames were lost (coverage stops counting).
const GAP_MS: u64 = 2_000;

pub struct LiveScanner {
    defs: Vec<LoadedDetector>,
    slots: Vec<Slot>,
    /// Frame size and game picture the slots were prepared for.
    prepared_for: Option<(u32, u32, Option<ContentRect>)>,
    crop: Option<ContentRect>,
    col_max: Vec<u8>,
    row_max: Vec<u8>,
    bar_frames: u32,
    bars_checked_at: Option<u64>,
    /// (unix ms, event)
    events: Vec<(u64, GameEvent)>,
    /// (unix ms) of every frame looked at, for coverage.
    seen: Vec<u64>,
}

impl LiveScanner {
    pub fn new(detectors: Vec<LoadedDetector>) -> Self {
        Self {
            defs: detectors.into_iter().filter(|d| live_capable(&d.def)).collect(),
            slots: Vec::new(),
            prepared_for: None,
            crop: None,
            col_max: Vec::new(),
            row_max: Vec::new(),
            bar_frames: 0,
            bars_checked_at: None,
            events: Vec::new(),
            seen: Vec::new(),
        }
    }

    pub fn from_config(cfg: &LiveConfig) -> Self {
        Self::new(
            cfg.detectors
                .iter()
                .map(|d| LoadedDetector {
                    def: d.def.clone(),
                    base_dir: d.base_dir.clone().into(),
                    user: false,
                })
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Look at one frame (the whole picture, grey, at the analysis height) taken at `unix_ms`.
    pub fn push(&mut self, unix_ms: u64, frame: &GrayImage) {
        if self.defs.is_empty() {
            return;
        }
        self.track_bars(unix_ms, frame);
        let cropped;
        let pic: &GrayImage = match self.crop {
            Some(c) => {
                cropped = image::imageops::crop_imm(frame, c.x, c.y, c.w, c.h).to_image();
                &cropped
            }
            None => frame,
        };
        let key = (frame.width(), frame.height(), self.crop);
        if self.prepared_for != Some(key) {
            self.prepare(pic.width(), pic.height());
            self.prepared_for = Some(key);
        }
        self.seen.push(unix_ms);
        for s in self.slots.iter_mut() {
            if s.last_sample.is_some_and(|l| unix_ms < l + s.period_ms * 9 / 10) {
                continue;
            }
            // After a gap (lost frames), what is on screen now starts a new run.
            if s.last_sample.is_some_and(|l| unix_ms > l + GAP_MS) {
                s.trig.gap();
            }
            s.last_sample = Some(unix_ms);
            let (score, reference) = s.det.score(FrameRef::Gray(pic));
            if !s.det.def.blocks.is_empty() {
                if score >= s.det.def.threshold {
                    match s.spans.last_mut() {
                        Some(last) if unix_ms <= last.1 + s.period_ms * 2 => last.1 = unix_ms,
                        _ => s.spans.push((unix_ms, unix_ms)),
                    }
                }
                continue;
            }
            let source = || EventSource::Vision {
                detector: s.det.def.id.clone(),
            };
            if let Some(vt) = s.value.as_mut() {
                if let Some(ch) = vt.push(unix_ms, score, s.det.value_of(reference)) {
                    for _ in 0..ch.count {
                        self.events.push((
                            ch.t_ms,
                            GameEvent::new(s.det.def.event.clone(), 0, source())
                                .with_confidence(score.max(0.0))
                                .with_meta(serde_json::json!({ "value": ch.value, "previous": ch.previous })),
                        ));
                    }
                }
                continue;
            }
            if let Some((at, peak)) = s.trig.push(unix_ms, score) {
                self.events
                    .push((at, GameEvent::new(s.det.def.event.clone(), 0, source()).with_confidence(peak)));
            }
        }
    }

    fn prepare(&mut self, w: u32, h: u32) {
        self.slots = self
            .defs
            .iter()
            .filter_map(|d| match PreparedDetector::load(d, w, h) {
                Ok(det) => Some(Slot {
                    trig: Trigger::new(&det.def),
                    value: det.def.is_value_detector().then(|| ValueTrigger::new(&det.def)),
                    period_ms: (1000.0 / d.def.sample_fps.max(0.1)) as u64,
                    last_sample: None,
                    spans: Vec::new(),
                    det,
                }),
                Err(e) => {
                    log::warn!("live detector {} not loaded: {e:#}", d.def.id);
                    None
                }
            })
            .collect();
    }

    /// Black bars around the game picture (a 16:9 game on an ultrawide screen), found from the
    /// brightest pixel of each column and row over a couple dozen frames.
    fn track_bars(&mut self, unix_ms: u64, frame: &GrayImage) {
        let due = self.bars_checked_at.is_none_or(|t| unix_ms >= t + BAR_RECHECK_MS);
        if !due {
            return;
        }
        let (w, h) = frame.dimensions();
        if self.col_max.len() != w as usize || self.row_max.len() != h as usize {
            self.col_max = vec![0; w as usize];
            self.row_max = vec![0; h as usize];
            self.bar_frames = 0;
        }
        for (y, row) in frame.as_raw().chunks_exact(w as usize).enumerate() {
            for (x, &v) in row.iter().enumerate() {
                if v > self.col_max[x] {
                    self.col_max[x] = v;
                }
                if v > self.row_max[y] {
                    self.row_max[y] = v;
                }
            }
        }
        self.bar_frames += 1;
        if self.bar_frames < BAR_FRAMES {
            return;
        }
        let new = bars_from_maxima(&self.col_max, &self.row_max);
        // A dark scene (loading screen) can look like one big bar: only trust a sensible result.
        if let Some(c) = new {
            if c.w >= w / 2 && c.h >= h / 2 {
                self.crop = Some(c).filter(|c| c.w < w || c.h < h);
            }
        } else {
            self.crop = None;
        }
        self.bars_checked_at = Some(unix_ms);
        self.col_max.fill(0);
        self.row_max.fill(0);
        self.bar_frames = 0;
    }

    /// The events of a recording that ran from `from_ms` to `to_ms` (unix), with times from its
    /// start, after blockers. Older events are forgotten.
    pub fn take(&mut self, from_ms: u64, to_ms: u64) -> LiveScan {
        let blocks: Vec<Blocker> = self
            .slots
            .iter()
            .filter(|s| !s.det.def.blocks.is_empty())
            .map(|s| Blocker {
                kinds: s.det.def.blocks.as_slice(),
                margin_ms: s.det.def.block_margin_ms,
                spans: s.spans.clone(),
            })
            .collect();
        let events: Vec<GameEvent> = self
            .events
            .iter()
            .filter(|(t, _)| *t >= from_ms && *t <= to_ms)
            .map(|(t, e)| {
                let mut e = e.clone();
                e.t_ms = t - from_ms;
                e
            })
            .collect();
        // Blocker spans in the same time base as the events.
        let blocks: Vec<Blocker> = blocks
            .into_iter()
            .map(|b| Blocker {
                spans: b
                    .spans
                    .iter()
                    .map(|(a, z)| (a.saturating_sub(from_ms), z.saturating_sub(from_ms)))
                    .collect(),
                ..b
            })
            .collect();
        let (mut kept, _) = apply_blockers(events, &blocks);
        kept.sort_by_key(|e| e.t_ms);
        let frames = self.seen.iter().filter(|t| **t >= from_ms && **t <= to_ms).count() as u64;
        let coverage = coverage(&self.seen, from_ms, to_ms);
        let scan = LiveScan {
            events: kept,
            detectors: self.slots.iter().map(|s| s.det.def.id.clone()).collect(),
            frames,
            coverage,
        };
        self.forget_before(to_ms);
        scan
    }

    /// Drop what happened before `unix_ms` (keeps memory flat over a long session).
    pub fn forget_before(&mut self, unix_ms: u64) {
        self.events.retain(|(t, _)| *t >= unix_ms);
        self.seen.retain(|t| *t >= unix_ms);
        for s in self.slots.iter_mut() {
            s.spans.retain(|(_, z)| *z + s.det.def.block_margin_ms >= unix_ms);
        }
    }
}

/// Share of `[from, to]` covered by frames no more than [`GAP_MS`] apart.
fn coverage(seen: &[u64], from: u64, to: u64) -> f32 {
    if to <= from {
        return 0.0;
    }
    let mut covered = 0u64;
    let mut prev: Option<u64> = None;
    for &t in seen.iter().filter(|t| **t >= from && **t <= to) {
        if let Some(p) = prev {
            if t - p <= GAP_MS {
                covered += t - p;
            }
        } else if t - from <= GAP_MS {
            covered += t - from;
        }
        prev = Some(t);
    }
    if let Some(p) = prev {
        if to - p <= GAP_MS {
            covered += to - p;
        }
    }
    (covered as f32 / (to - from) as f32).min(1.0)
}

/// The game picture from per-column and per-row maxima: everything brighter than the bar limit,
/// ignoring bars thinner than 2% (or none at all).
pub fn bars_from_maxima(col_max: &[u8], row_max: &[u8]) -> Option<ContentRect> {
    let span = |m: &[u8]| -> Option<(usize, usize)> {
        let first = m.iter().position(|&v| v > BAR_LIMIT)?;
        let last = m.iter().rposition(|&v| v > BAR_LIMIT)?;
        Some((first, last))
    };
    let (x0, x1) = span(col_max)?;
    let (y0, y1) = span(row_max)?;
    let (w, h) = (col_max.len(), row_max.len());
    let snap = |a: usize, b: usize, n: usize| {
        // Bars under 2% of the size are noise (a dark HUD edge), not bars.
        if a * 50 < n && (n - 1 - b) * 50 < n {
            (0, n)
        } else {
            (a & !1, ((b + 1 - (a & !1)) + 1) & !1)
        }
    };
    let (x, cw) = snap(x0, x1, w);
    let (y, ch) = snap(y0, y1, h);
    Some(ContentRect {
        x: x as u32,
        y: y as u32,
        w: cw.min(w - x) as u32,
        h: ch.min(h - y) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::DetectorDef;

    #[test]
    fn bars_and_coverage() {
        // 100 columns: 12 dark on each side.
        let mut cols = vec![200u8; 100];
        cols[..12].fill(5);
        cols[88..].fill(5);
        let rows = vec![200u8; 40];
        assert_eq!(bars_from_maxima(&cols, &rows), Some(ContentRect { x: 12, y: 0, w: 76, h: 40 }));
        // A single dark column at the edge is not a bar.
        let mut cols = vec![200u8; 100];
        cols[0] = 3;
        assert_eq!(bars_from_maxima(&cols, &rows), Some(ContentRect { x: 0, y: 0, w: 100, h: 40 }));
        assert_eq!(bars_from_maxima(&[0; 10], &rows), None);

        let seen: Vec<u64> = (0..=100).map(|i| 1000 + i * 125).collect(); // 1.0 .. 13.5 s
        assert!((coverage(&seen, 1000, 13_500) - 1.0).abs() < 1e-3);
        // A 5 s hole in the middle.
        let holed: Vec<u64> = seen.iter().copied().filter(|t| !(5000..10_000).contains(t)).collect();
        let c = coverage(&holed, 1000, 13_500);
        assert!(c > 0.55 && c < 0.65, "{c}");
    }

    #[test]
    fn finds_a_banner_live() {
        // A bright square on a dark background, shown from 2.0 s to 3.5 s.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("templates")).unwrap();
        let tpl = GrayImage::from_fn(40, 40, |x, y| {
            image::Luma([if (8..32).contains(&x) && (8..32).contains(&y) && (x + y) % 7 != 0 {
                230
            } else {
                20
            }])
        });
        tpl.save(dir.path().join("templates/sq.png")).unwrap();
        let def: DetectorDef = serde_json::from_value(serde_json::json!({
            "id": "sq", "label": "Square", "event": "kill",
            "region": { "x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5 },
            "templates": ["templates/sq.png"], "templateFrameHeight": 180,
            "threshold": 0.8, "cooldownMs": 1000, "minConsecutive": 2, "sampleFps": 8
        }))
        .unwrap();
        let mut live = LiveScanner::new(vec![LoadedDetector {
            def,
            base_dir: dir.path().to_path_buf(),
            user: false,
        }]);
        let start = 1_700_000_000_000u64;
        for i in 0..48u64 {
            let t = start + i * 125;
            let on = (2000..3500).contains(&(i * 125));
            let mut f = GrayImage::from_pixel(320, 180, image::Luma([20]));
            // Some texture so the background is not flat.
            for (x, y, p) in f.enumerate_pixels_mut() {
                p.0[0] = 20 + ((x * 7 + y * 3) % 11) as u8;
            }
            if on {
                image::imageops::overlay(&mut f, &tpl, 140, 70);
            }
            live.push(t, &f);
        }
        let scan = live.take(start + 500, start + 6_000);
        assert_eq!(scan.events.len(), 1, "{:?}", scan.events);
        assert_eq!(scan.events[0].kind, "kill");
        assert_eq!(scan.events[0].t_ms, 1500, "2.0 s after a start at 0.5 s");
        assert_eq!(scan.detectors, vec!["sq".to_string()]);
        assert!(scan.coverage > 0.9);
    }
}
