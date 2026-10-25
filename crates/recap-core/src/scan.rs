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
    /// Decode every frame instead of the two-pass scan (keyframes first, then only the seconds
    /// around candidates). Detector Studio's graphs use this; so does an old recording whose
    /// keyframes are too far apart.
    pub full: bool,
}

impl Default for ScanOptions<'_> {
    fn default() -> Self {
        Self {
            analysis_height: 720,
            keep_traces: false,
            cancel: None,
            progress: None,
            content: None,
            full: false,
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
    if !opts.full && !opts.keep_traces {
        let gop = ff.keyframe_interval_ms(video);
        match gop.filter(|g| *g <= MAX_FAST_GOP_MS) {
            Some(gop) => {
                let started = std::time::Instant::now();
                if let Some(r) = scan_two_pass(ff, video, &ready, &info, crop, color, fps, gop, &opts)? {
                    log::info!(
                        "{}: two-pass scan, keyframes every {gop} ms: {} frames decoded, {} events, {:.1} s",
                        video.display(),
                        r.frames_decoded,
                        r.events.len(),
                        started.elapsed().as_secs_f32()
                    );
                    return Ok(r);
                }
            }
            None => log::info!("{}: keyframes {:?} ms apart; scanning every frame", video.display(), gop),
        }
    }
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
        let scores = score_all(
            &slots,
            |s| i.is_multiple_of(s.every),
            |s| s.det.score(frame_ref(&frame, &gray, s.det.needs_color())),
        );
        for (s, sc) in slots.iter_mut().zip(scores) {
            let Some((score, reference)) = sc else { continue };
            let fr = frame_ref(&frame, &gray, s.det.needs_color());
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

/// The frame a detector looks at: colour if it filters by colour, else the shared grey copy.
fn frame_ref<'a>(frame: &'a AnalysisFrame, gray: &'a Option<image::GrayImage>, color: bool) -> FrameRef<'a> {
    match (frame, gray) {
        (AnalysisFrame::Gray(g), _) => FrameRef::Gray(g),
        (AnalysisFrame::Rgb(_), Some(g)) if !color => FrameRef::Gray(g),
        (AnalysisFrame::Rgb(c), _) => FrameRef::Rgb(c),
    }
}

/// Threads for scoring detectors on one frame: just this one while a game is being played
/// (the game keeps the CPU), otherwise up to four, leaving a core free.
fn score_threads(items: usize) -> usize {
    if crate::ffmpeg::game_running() {
        return 1;
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    cores.saturating_sub(1).clamp(1, 4).min(items.max(1))
}

/// `f` for every item where `due`, spread over [`score_threads`] threads.
fn score_all<T: Sync>(items: &[T], due: impl Fn(&T) -> bool + Sync, f: impl Fn(&T) -> (f32, usize) + Sync) -> Vec<Option<(f32, usize)>> {
    score_all_idx(items, |_, t| due(t), f)
}

fn score_all_idx<T: Sync>(
    items: &[T],
    due: impl Fn(usize, &T) -> bool + Sync,
    f: impl Fn(&T) -> (f32, usize) + Sync,
) -> Vec<Option<(f32, usize)>> {
    let jobs: Vec<usize> = (0..items.len()).filter(|&i| due(i, &items[i])).collect();
    let mut out = vec![None; items.len()];
    let threads = score_threads(jobs.len());
    if threads <= 1 {
        for i in jobs {
            out[i] = Some(f(&items[i]));
        }
        return out;
    }
    let f = &f;
    let results: Vec<(usize, (f32, usize))> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                // Interleaved so heavy and light detectors spread over the threads.
                let mine: Vec<usize> = jobs.iter().copied().skip(k).step_by(threads).collect();
                sc.spawn(move || mine.into_iter().map(|i| (i, f(&items[i]))).collect::<Vec<_>>())
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    for (i, r) in results {
        out[i] = Some(r);
    }
    out
}

/// Keyframes further apart than this can miss a short on-screen element; such recordings get
/// the full scan.
pub const MAX_FAST_GOP_MS: u64 = 2_000;

/// A keyframe scoring this far below a detector's threshold is still worth a closer look.
fn candidate_threshold(threshold: f32) -> f32 {
    threshold - (threshold * 0.15).max(0.08)
}

/// Pass 1 decodes keyframes only and notes where each detector comes close to matching;
/// pass 2 decodes just the seconds around those moments at the detectors' own sample rate and
/// fires events exactly as the full scan would. Blockers and value detectors use the keyframes.
/// Returns None when pass 1 saw too little of the video to trust (then the caller scans fully).
#[allow(clippy::too_many_arguments)]
fn scan_two_pass(
    ff: &Ffmpeg,
    video: &Path,
    ready: &[&LoadedDetector],
    info: &crate::ffmpeg::VideoInfo,
    crop: Option<ContentRect>,
    color: bool,
    fps: f32,
    gop: u64,
    opts: &ScanOptions,
) -> Result<Option<ScanResult>> {
    use crate::ffmpeg::Decode;
    let cancelled = || opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    // Sample the keyframe stream on a clock four times faster than the keyframes, so every
    // keyframe shows up even when they come at uneven times; repeats are skipped below.
    let key_fps = (4000.0 / gop as f32).clamp(1.0, 8.0);
    let mut frames = ff.analysis_frames_opts(
        video,
        key_fps,
        info,
        opts.analysis_height,
        crop,
        color,
        Decode {
            range: None,
            keyframes_only: true,
        },
    )?;
    let (w, h) = (frames.width, frames.height);
    struct Slot {
        det: PreparedDetector,
        trig: Trigger,
        value: Option<ValueTrigger>,
        every: u64,
        cand: Vec<u64>,
        spans: Vec<(u64, u64)>,
    }
    let mut slots = Vec::new();
    for d in ready {
        match PreparedDetector::load(d, w, h) {
            Ok(det) => slots.push(Slot {
                trig: Trigger::new(&det.def),
                value: det.def.is_value_detector().then(|| ValueTrigger::new(&det.def)),
                every: ((fps / d.def.sample_fps).round() as u64).max(1),
                det,
                cand: Vec::new(),
                spans: Vec::new(),
            }),
            Err(e) => log::warn!("detector {} not loaded: {e:#}", d.def.id),
        }
    }
    let mut result = ScanResult::default();
    let score_frame =
        |s: &Slot, frame: &AnalysisFrame, gray: &Option<image::GrayImage>| s.det.score(frame_ref(frame, gray, s.det.needs_color()));
    let gray_of = |frame: &AnalysisFrame| match frame {
        AnalysisFrame::Rgb(c) => Some(image::imageops::grayscale(c)),
        _ => None,
    };

    // Pass 1: keyframes.
    let mut last_t = 0u64;
    let mut n_key = 0u64;
    let mut last_progress_s = 0u64;
    let mut prev_raw: Option<Vec<u8>> = None;
    while let Some((t_ms, frame)) = frames.next_frame() {
        if cancelled() {
            result.cancelled = true;
            return Ok(Some(result));
        }
        // The same keyframe again (the clock ticked between keyframes): nothing new to score.
        let raw: &[u8] = match &frame {
            AnalysisFrame::Gray(g) => g.as_raw(),
            AnalysisFrame::Rgb(c) => c.as_raw(),
        };
        if prev_raw.as_deref() == Some(raw) {
            continue;
        }
        prev_raw = Some(raw.to_vec());
        n_key += 1;
        last_t = t_ms;
        result.frames_decoded += 1;
        let gray = gray_of(&frame);
        let scores = score_all(&slots, |_| true, |s| score_frame(s, &frame, &gray));
        for (s, sc) in slots.iter_mut().zip(scores) {
            let Some((score, reference)) = sc else { continue };
            if !s.det.def.blocks.is_empty() {
                if score >= s.det.def.threshold {
                    match s.spans.last_mut() {
                        Some(last) if t_ms <= last.1 + gop * 2 => last.1 = t_ms,
                        _ => s.spans.push((t_ms, t_ms)),
                    }
                }
                continue;
            }
            if let Some(vt) = s.value.as_mut() {
                if let Some(ch) = vt.push(t_ms, score, s.det.value_of(reference)) {
                    for _ in 0..ch.count {
                        result.events.push(
                            GameEvent::new(
                                s.det.def.event.clone(),
                                ch.t_ms,
                                EventSource::Vision {
                                    detector: s.det.def.id.clone(),
                                },
                            )
                            .with_confidence(score.max(0.0))
                            .with_meta(serde_json::json!({ "value": ch.value, "previous": ch.previous })),
                        );
                    }
                }
                continue;
            }
            if std::env::var("RECAP_SCAN_DEBUG_DET").is_ok_and(|d| d == "*" || d == s.det.def.id) {
                eprintln!("  key {t_ms} {} {score:.3}", s.det.def.id);
            }
            if score >= candidate_threshold(s.det.def.threshold) {
                s.cand.push(t_ms);
            }
        }
        if let Some(p) = opts.progress {
            if t_ms / 1000 > last_progress_s + 5 && info.duration_ms > 0 {
                last_progress_s = t_ms / 1000;
                p((t_ms as f32 / info.duration_ms as f32 * 0.5).min(0.5));
            }
        }
    }
    drop(frames);
    // The keyframes must cover the video; otherwise something is off (variable keyframes,
    // a damaged file): let the full scan decide.
    let expected = info.duration_ms / gop.max(1);
    if info.duration_ms > 10_000 && (last_t + gop * 3 < info.duration_ms || n_key * 2 < expected) {
        log::info!(
            "{}: keyframe pass covered {last_t} of {} ms ({n_key} frames); scanning every frame",
            video.display(),
            info.duration_ms
        );
        return Ok(None);
    }

    // Pass 2: the seconds around candidates, in time order. A candidate that could only
    // produce an event inside a detector's cooldown after one it already fired is skipped (the
    // full scan would drop that event too), so long-lasting elements cost one window each.
    let pre = gop + 500;
    let post = 2 * gop + 1000;
    let debug = std::env::var_os("RECAP_SCAN_DEBUG").is_some();
    let mut next = vec![0usize; slots.len()];
    let mut last_fire: Vec<Option<u64>> = vec![None; slots.len()];
    let mut covered_to = 0u64;
    let (mut n_windows, mut window_ms) = (0usize, 0u64);
    // Where a candidate's window ends (never past the end of the video).
    let end_of = |c: u64| (c + post).min(info.duration_ms.max(c + 1));
    loop {
        if cancelled() {
            result.cancelled = true;
            break;
        }
        for (k, s) in slots.iter().enumerate() {
            while let Some(&c) = s.cand.get(next[k]) {
                let suppressed = last_fire[k].is_some_and(|l| c + post < l + s.det.def.cooldown_ms);
                if end_of(c) <= covered_to || suppressed {
                    next[k] += 1;
                } else {
                    break;
                }
            }
        }
        let Some(c0) = slots.iter().enumerate().filter_map(|(k, s)| s.cand.get(next[k]).copied()).min() else {
            break;
        };
        let a = c0.saturating_sub(pre).max(covered_to);
        // Chain candidates that start before this window ends into it.
        let mut b = end_of(c0);
        loop {
            let ext = slots
                .iter()
                .enumerate()
                .flat_map(|(k, s)| s.cand[next[k]..].iter().copied())
                .filter(|&c| c.saturating_sub(pre) <= b + 250 && end_of(c) > b)
                .max();
            match ext {
                Some(c) => b = end_of(c),
                None => break,
            }
        }
        // Only the detectors with a candidate around here look.
        let active: Vec<bool> = slots
            .iter()
            .enumerate()
            .map(|(k, s)| {
                s.det.def.blocks.is_empty()
                    && s.value.is_none()
                    && s.cand[next[k]..].iter().any(|&c| c + post >= a && c.saturating_sub(pre) <= b)
            })
            .collect();
        let continues = a <= covered_to + 250 && covered_to > 0;
        covered_to = b;
        if !active.iter().any(|x| *x) || b <= a {
            continue;
        }
        n_windows += 1;
        window_ms += b - a;
        let mut wf = ff.analysis_frames_opts(
            video,
            fps,
            info,
            opts.analysis_height,
            crop,
            color,
            Decode {
                range: Some((a, b - a)),
                keyframes_only: false,
            },
        )?;
        if !continues {
            for s in slots.iter_mut() {
                s.trig.gap();
            }
        }
        let mut i = 0u64;
        while let Some((t_ms, frame)) = wf.next_frame() {
            result.frames_decoded += 1;
            let gray = gray_of(&frame);
            let due: Vec<bool> = slots.iter().zip(&active).map(|(s, on)| *on && i.is_multiple_of(s.every)).collect();
            let scores = score_all_idx(&slots, |k, _| due[k], |s| score_frame(s, &frame, &gray));
            for (k, (s, sc)) in slots.iter_mut().zip(scores).enumerate() {
                let Some((score, _)) = sc else { continue };
                if let Some((at, peak)) = s.trig.push(t_ms, score) {
                    last_fire[k] = Some(at);
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
            i += 1;
        }
        if let Some(p) = opts.progress {
            if info.duration_ms > 0 {
                p(0.5 + 0.5 * (b as f32 / info.duration_ms as f32).min(1.0));
            }
        }
    }
    if debug {
        eprintln!("pass1: {n_key} keyframes; pass2: {n_windows} windows, {window_ms} ms");
        for s in &slots {
            eprintln!("  {}: {} candidates", s.det.def.id, s.cand.len());
        }
    }

    let blocks: Vec<Blocker> = slots
        .iter()
        .filter(|s| !s.det.def.blocks.is_empty())
        .map(|s| Blocker {
            kinds: s.det.def.blocks.as_slice(),
            margin_ms: s.det.def.block_margin_ms + gop,
            spans: s.spans.clone(),
        })
        .collect();
    if !blocks.is_empty() {
        let (kept, dropped) = apply_blockers(std::mem::take(&mut result.events), &blocks);
        result.events = kept;
        result.suppressed = dropped;
    }
    result.events.sort_by_key(|e| e.t_ms);
    Ok(Some(result))
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
    fn candidate_threshold_is_a_little_below_the_real_one() {
        assert!((candidate_threshold(0.6) - 0.51).abs() < 1e-6);
        assert!((candidate_threshold(0.9) - 0.765).abs() < 1e-6);
        assert!((candidate_threshold(0.4) - 0.32).abs() < 1e-6);
    }

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
