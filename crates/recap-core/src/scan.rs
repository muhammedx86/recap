//! Post-match scan: run every vision detector over a finished recording.
//!
//! The video is decoded once, at the highest sample rate any detector asks for,
//! scaled down to `analysis_height` and converted to grayscale by ffmpeg. Each
//! detector then only looks at its own region on the frames it is due to sample.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use serde::Serialize;

use crate::{
    events::{EventSource, GameEvent},
    ffmpeg::{AnalysisFrame, ContentRect, Ffmpeg},
    games::LoadedDetector,
    vision::{FrameRef, MatchLocation, PreparedDetector, Trigger, ValueTrigger},
};

/// Score samples for one detector, for the Detector Studio graph.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreTrace {
    pub detector: String,
    /// (t_ms, score) pairs.
    pub samples: Vec<(u64, f32)>,
    /// Where it matched when it fired: (t_ms of the event, location on the frame that fired it).
    pub hits: Vec<(u64, MatchLocation)>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub events: Vec<GameEvent>,
    pub traces: Vec<ScoreTrace>,
    pub frames_decoded: u64,
    pub cancelled: bool,
    /// Detections dropped because a blocker (spectating, killcam...) was on screen.
    pub suppressed: Vec<GameEvent>,
}

pub struct ScanOptions<'a> {
    pub analysis_height: u32,
    /// Keep per-sample scores (used by Detector Studio's test run).
    pub keep_traces: bool,
    pub cancel: Option<&'a AtomicBool>,
    /// Called with progress 0..1 roughly once per second of video.
    pub progress: Option<&'a (dyn Fn(f32) + Sync)>,
    /// The game picture, when already known (`Some(None)` = no black bars); detected otherwise.
    pub content: Option<Option<ContentRect>>,
}

impl Default for ScanOptions<'_> {
    fn default() -> Self {
        Self {
            analysis_height: 720,
            keep_traces: false,
            cancel: None,
            progress: None,
            content: None,
        }
    }
}

pub fn scan_video(ff: &Ffmpeg, video: &Path, detectors: &[LoadedDetector], opts: ScanOptions) -> Result<ScanResult> {
    let ready: Vec<&LoadedDetector> = detectors.iter().filter(|d| d.def.is_ready()).collect();
    if ready.is_empty() {
        return Ok(ScanResult::default());
    }
    let info = ff.probe(video)?;
    let fps = ready.iter().map(|d| d.def.sample_fps).fold(0.5f32, f32::max);
    // Measure regions inside the game picture, not the black bars around it.
    let crop = match opts.content {
        Some(c) => c,
        None => ff.content_rect(video, &info),
    };
    if let Some(c) = crop {
        log::info!(
            "{}: game picture is {}x{} at ({}, {}) inside {}x{}",
            video.display(),
            c.w,
            c.h,
            c.x,
            c.y,
            info.width,
            info.height
        );
    }
    // Colour frames only when a detector filters by colour (3x the data otherwise wasted).
    let color = ready.iter().any(|d| d.def.color_filter.is_some());
    let mut frames = ff.analysis_frames(video, fps, &info, opts.analysis_height, crop, color)?;
    let (w, h) = (frames.width, frames.height);

    struct Slot {
        det: PreparedDetector,
        trig: Trigger,
        value: Option<ValueTrigger>,
        every: u64,
        trace: Vec<(u64, f32)>,
        hits: Vec<(u64, MatchLocation)>,
        /// Blockers: [start, end] of the spans where the detector matched.
        spans: Vec<(u64, u64)>,
    }
    let mut slots = Vec::new();
    for d in ready {
        match PreparedDetector::load(d, w, h) {
            Ok(det) => {
                let every = ((fps / d.def.sample_fps).round() as u64).max(1);
                slots.push(Slot {
                    trig: Trigger::new(&det.def),
                    value: det.def.is_value_detector().then(|| ValueTrigger::new(&det.def)),
                    det,
                    every,
                    trace: Vec::new(),
                    hits: Vec::new(),
                    spans: Vec::new(),
                });
            }
            Err(e) => log::warn!("detector {} not loaded: {e:#}", d.def.id),
        }
    }
    let sample_ms = (1000.0 / fps).round() as u64;

    let mut result = ScanResult::default();
    let mut last_progress_s = 0u64;
    let mut i = 0u64;
    while let Some((t_ms, frame)) = frames.next_frame() {
        if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            result.cancelled = true;
            break;
        }
        result.frames_decoded += 1;
        // Grayscale detectors on a colour stream share one conversion per frame.
        let gray = match &frame {
            AnalysisFrame::Rgb(c) if slots.iter().any(|s| !s.det.needs_color() && i.is_multiple_of(s.every)) => {
                Some(image::imageops::grayscale(c))
            }
            _ => None,
        };
        for s in slots.iter_mut() {
            if !i.is_multiple_of(s.every) {
                continue;
            }
            let fr = match (&frame, &gray) {
                (AnalysisFrame::Gray(g), _) => FrameRef::Gray(g),
                (AnalysisFrame::Rgb(_), Some(g)) if !s.det.needs_color() => FrameRef::Gray(g),
                (AnalysisFrame::Rgb(c), _) => FrameRef::Rgb(c),
            };
            let (score, reference) = s.det.score(fr);
            if opts.keep_traces {
                s.trace.push((t_ms, score));
            }
            if !s.det.def.blocks.is_empty() && score >= s.det.def.threshold {
                // Extend the current span when this sample follows the last one.
                match s.spans.last_mut() {
                    Some(last) if t_ms <= last.1 + sample_ms * s.every + sample_ms => last.1 = t_ms,
                    _ => s.spans.push((t_ms, t_ms)),
                }
            }
            if !s.det.def.blocks.is_empty() {
                // A blocker only suppresses other detections; it marks nothing itself.
                continue;
            }
            let source = || EventSource::Vision {
                detector: s.det.def.id.clone(),
            };
            if let Some(vt) = s.value.as_mut() {
                if let Some(ch) = vt.push(t_ms, score, s.det.value_of(reference)) {
                    for _ in 0..ch.count {
                        result.events.push(
                            GameEvent::new(s.det.def.event.clone(), ch.t_ms, source())
                                .with_confidence(score.max(0.0))
                                .with_meta(serde_json::json!({ "value": ch.value, "previous": ch.previous })),
                        );
                    }
                }
                continue;
            }
            if let Some((at, peak)) = s.trig.push(t_ms, score) {
                if opts.keep_traces && s.hits.len() < 200 {
                    if let Some(loc) = s.det.locate(fr) {
                        s.hits.push((at, loc));
                    }
                }
                result
                    .events
                    .push(GameEvent::new(s.det.def.event.clone(), at, source()).with_confidence(peak));
            }
        }
        i += 1;
        if let Some(p) = opts.progress {
            if t_ms / 1000 > last_progress_s && info.duration_ms > 0 {
                last_progress_s = t_ms / 1000;
                p((t_ms as f32 / info.duration_ms as f32).min(1.0));
            }
        }
    }
    // Blockers: drop detections of the kinds they block while they were on screen.
    let blocks: Vec<Blocker> = slots
        .iter()
        .filter(|s| !s.det.def.blocks.is_empty())
        .map(|s| Blocker {
            kinds: s.det.def.blocks.as_slice(),
            margin_ms: s.det.def.block_margin_ms,
            spans: s.spans.clone(),
        })
        .collect();
    if !blocks.is_empty() {
        let (kept, dropped) = apply_blockers(std::mem::take(&mut result.events), &blocks);
        result.events = kept;
        result.suppressed = dropped;
    }
    if opts.keep_traces {
        result.traces = slots
            .into_iter()
            .map(|s| ScoreTrace {
                detector: s.det.def.id,
                samples: s.trace,
                hits: s.hits,
            })
            .collect();
    }
    result.events.sort_by_key(|e| e.t_ms);
    Ok(result)
}

/// Where a blocker detector was on screen, and what it hides.
pub struct Blocker<'a> {
    pub kinds: &'a [String],
    pub margin_ms: u64,
    /// [start, end] ms of each on-screen span.
    pub spans: Vec<(u64, u64)>,
}

/// Split `events` into (kept, dropped): an event is dropped when one of the blockers lists its
/// kind and was on screen within its margin of the event.
pub fn apply_blockers(events: Vec<GameEvent>, blocks: &[Blocker]) -> (Vec<GameEvent>, Vec<GameEvent>) {
    events.into_iter().partition(|e| {
        !blocks.iter().any(|b| {
            b.kinds.contains(&e.kind)
                && b.spans
                    .iter()
                    .any(|(start, end)| e.t_ms + b.margin_ms >= *start && e.t_ms <= end + b.margin_ms)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blockers_drop_only_their_kinds_near_their_spans() {
        let ev = |k: &str, t: u64| GameEvent::new(k, t, EventSource::Vision { detector: "d".into() });
        let kinds = vec!["kill".to_string()];
        let blocks = [Blocker {
            kinds: &kinds,
            margin_ms: 1000,
            spans: vec![(10_000, 20_000)],
        }];
        let (kept, dropped) = apply_blockers(
            vec![
                ev("kill", 5_000),
                ev("kill", 9_500),
                ev("kill", 15_000),
                ev("death", 15_000),
                ev("kill", 21_500),
            ],
            &blocks,
        );
        let t = |v: &[GameEvent]| v.iter().map(|e| (e.kind.clone(), e.t_ms)).collect::<Vec<_>>();
        assert_eq!(
            t(&kept),
            vec![("kill".into(), 5_000), ("death".into(), 15_000), ("kill".into(), 21_500)]
        );
        assert_eq!(t(&dropped), vec![("kill".into(), 9_500), ("kill".into(), 15_000)]);
    }
}
