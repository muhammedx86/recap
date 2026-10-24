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
    ffmpeg::{ContentRect, Ffmpeg},
    games::LoadedDetector,
    vision::{MatchLocation, PreparedDetector, Trigger},
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
    let frames = ff.gray_frames(video, fps, &info, opts.analysis_height, crop)?;
    let (w, h) = (frames.width, frames.height);

    struct Slot {
        det: PreparedDetector,
        trig: Trigger,
        every: u64,
        trace: Vec<(u64, f32)>,
        hits: Vec<(u64, MatchLocation)>,
    }
    let mut slots = Vec::new();
    for d in ready {
        match PreparedDetector::load(d, w, h) {
            Ok(det) => {
                let every = ((fps / d.def.sample_fps).round() as u64).max(1);
                slots.push(Slot {
                    trig: Trigger::new(&det.def),
                    det,
                    every,
                    trace: Vec::new(),
                    hits: Vec::new(),
                });
            }
            Err(e) => log::warn!("detector {} not loaded: {e:#}", d.def.id),
        }
    }

    let mut result = ScanResult::default();
    let mut last_progress_s = 0u64;
    for (i, (t_ms, frame)) in frames.enumerate() {
        if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            result.cancelled = true;
            break;
        }
        result.frames_decoded += 1;
        for s in slots.iter_mut() {
            if !(i as u64).is_multiple_of(s.every) {
                continue;
            }
            let score = s.det.score_frame(&frame);
            if opts.keep_traces {
                s.trace.push((t_ms, score));
            }
            if let Some((at, peak)) = s.trig.push(t_ms, score) {
                if opts.keep_traces && s.hits.len() < 200 {
                    if let Some(loc) = s.det.locate_frame(&frame) {
                        s.hits.push((at, loc));
                    }
                }
                result.events.push(
                    GameEvent::new(
                        s.det.def.event.clone(),
                        at,
                        EventSource::Vision {
                            detector: s.det.def.id.clone(),
                        },
                    )
                    .with_confidence(peak),
                );
            }
        }
        if let Some(p) = opts.progress {
            if t_ms / 1000 > last_progress_s && info.duration_ms > 0 {
                last_progress_s = t_ms / 1000;
                p((t_ms as f32 / info.duration_ms as f32).min(1.0));
            }
        }
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
