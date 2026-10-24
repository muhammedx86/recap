//! Detector Studio backend: load frames, cut templates, test detectors.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::SystemTime,
};

use anyhow::{bail, Context, Result};
use base64::Engine;
use image::{DynamicImage, ImageFormat};
use recap_core::{
    events::GameEvent,
    ffmpeg::ContentRect,
    games::{
        ensure_templates_copied, save_user_detector, save_user_reader, ChoiceOption, DetectorDef, LoadedDetector, LoadedReader, ReaderDef,
        Region,
    },
    readers::{read_details, PreparedReader, ReadWindows},
    scan::{scan_video, ScanOptions},
    vision::MatchLocation,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FrameSource {
    Video { path: String, t_ms: u64 },
    Image { path: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameResult {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestHit {
    pub t_ms: u64,
    #[serde(flatten)]
    pub location: MatchLocation,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub events: Vec<GameEvent>,
    pub samples: Vec<(u64, f32)>,
    pub duration_ms: u64,
    /// Where it matched for each detection (box as fractions of the game picture).
    pub hits: Vec<TestHit>,
    /// The run was cancelled; samples cover only part of the recording.
    pub cancelled: bool,
}

/// The game picture inside a recording, in video pixels.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// What Studio needs to show a recording: its size, length and where the game picture is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfoResult {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    /// The game picture when the recording has black bars around it; null when it fills the frame.
    pub content: Option<PictureRect>,
}

struct CachedInfo {
    stamp: (Option<SystemTime>, u64),
    info: VideoInfoResult,
    crop: Option<ContentRect>,
}

fn info_cache() -> &'static Mutex<HashMap<PathBuf, CachedInfo>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedInfo>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Size, length and game picture of a recording. Black-bar detection runs ffmpeg for a few
/// seconds, so results are cached per file (until the file changes).
pub fn video_info(st: &AppState, path: &Path) -> Result<(VideoInfoResult, Option<ContentRect>)> {
    let meta = std::fs::metadata(path).with_context(|| format!("recording not found: {}", path.display()))?;
    let stamp = (meta.modified().ok(), meta.len());
    if let Some(c) = info_cache().lock().unwrap().get(path) {
        if c.stamp == stamp {
            return Ok((c.info.clone(), c.crop));
        }
    }
    let info = st.ff.probe(path)?;
    let crop = st.ff.content_rect(path, &info);
    let result = VideoInfoResult {
        width: info.width,
        height: info.height,
        duration_ms: info.duration_ms,
        content: crop.map(|c| PictureRect {
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
        }),
    };
    info_cache().lock().unwrap().insert(
        path.to_path_buf(),
        CachedInfo {
            stamp,
            info: result.clone(),
            crop,
        },
    );
    Ok((result, crop))
}

/// Set by `studio_cancel`; checked by running Studio tests.
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancel_running() {
    CANCEL.store(true, Ordering::Relaxed);
}

fn load_image(st: &AppState, source: &FrameSource) -> Result<DynamicImage> {
    match source {
        FrameSource::Video { path, t_ms } => {
            // Show (and cut templates from) the game picture only, so boxes line up with scans.
            let video = Path::new(path);
            let crop = video_info(st, video).map(|(_, c)| c).unwrap_or(None);
            let png = st.ff.frame_png(video, *t_ms, crop)?;
            Ok(image::load_from_memory_with_format(&png, ImageFormat::Png)?)
        }
        FrameSource::Image { path } => Ok(image::open(path).with_context(|| format!("opening {path}"))?),
    }
}

pub fn load_frame(st: &AppState, source: &FrameSource) -> Result<FrameResult> {
    let img = load_image(st, source)?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(FrameResult {
        data_url: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)),
        width: img.width(),
        height: img.height(),
    })
}

/// Save a detector into the user's game folder, optionally cutting a new template from the frame.
pub fn save_detector(st: &AppState, game_id: &str, mut det: DetectorDef, source: &FrameSource, template_box: Option<Region>) -> Result<()> {
    let game = st.game(game_id).context("unknown game")?;
    let user_game_dir = st.paths.user_games.join(game_id);
    // Templates that came from the bundled folder must be copied so the user detector can find them.
    if let Some(existing) = game.detectors.iter().find(|d| d.def.id == det.id) {
        ensure_templates_copied(&det, &existing.base_dir, &user_game_dir)?;
    }
    let mut new_template = None;
    if let Some(bx) = template_box {
        bx.validate()?;
        let img = load_image(st, source)?;
        let (x, y, w, h) = bx.to_pixels(img.width(), img.height());
        if w < 6 || h < 6 {
            bail!("the selected box is too small; drag a larger rectangle");
        }
        let mut crop = img.crop_imm(x, y, w, h);
        let has_existing = !det.templates.is_empty() && det.template_frame_height > 0;
        if has_existing && det.template_frame_height != img.height() {
            // All templates of a detector share one reference height: rescale this crop to it.
            let f = det.template_frame_height as f32 / img.height() as f32;
            let nw = ((w as f32 * f).round() as u32).max(4);
            let nh = ((h as f32 * f).round() as u32).max(4);
            crop = crop.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
        }
        let mut png = Vec::new();
        crop.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)?;
        let name = format!("{}-{}.png", det.id, chrono::Local::now().format("%Y%m%d-%H%M%S"));
        det.templates.push(format!("templates/{name}"));
        if !has_existing {
            det.template_frame_height = img.height();
        }
        det.template_box = Some(bx);
        new_template = Some((name, png));
    }
    if det.label.trim().is_empty() {
        det.label = det.id.replace(['-', '_'], " ");
    }
    save_user_detector(
        &st.paths.user_games,
        &game.def,
        &det,
        new_template.as_ref().map(|(n, b)| (n.as_str(), b.as_slice())),
    )?;
    Ok(())
}

/// Where the templates of `det` can be found: user folder first, then the loaded game folders.
fn template_base(st: &AppState, game_id: &str, det: &DetectorDef) -> PathBuf {
    let mut candidates = vec![st.paths.user_games.join(game_id)];
    if let Some(g) = st.game(game_id) {
        candidates.push(g.dir.clone());
        candidates.extend(g.detectors.iter().map(|d| d.base_dir.clone()));
    }
    candidates.push(st.paths.bundled_games.join(game_id));
    candidates
        .iter()
        .find(|c| det.templates.iter().all(|t| c.join(t).exists()))
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

/// Run an (unsaved) detector over a recording and return its score curve.
pub fn test_detector(st: &AppState, game_id: &str, det: DetectorDef, match_path: &Path) -> Result<TestResult> {
    test_detector_with_progress(st, game_id, det, match_path, &|_| {})
}

/// [`test_detector`], reporting progress 0..1; `studio_cancel` stops it early.
pub fn test_detector_with_progress(
    st: &AppState,
    game_id: &str,
    det: DetectorDef,
    match_path: &Path,
    progress: &(dyn Fn(f32) + Sync),
) -> Result<TestResult> {
    det.validate()?;
    if det.templates.is_empty() {
        bail!("this detector has no reference image yet: draw a box and save first");
    }
    let base_dir = template_base(st, game_id, &det);
    let mut d = det.clone();
    d.enabled = true;
    let loaded = LoadedDetector {
        def: d.clone(),
        base_dir,
        user: true,
    };
    let (info, crop) = video_info(st, match_path)?;
    let analysis_height = st.settings.read().unwrap().analysis_height;
    CANCEL.store(false, Ordering::Relaxed);
    let res = scan_video(
        &st.ff,
        match_path,
        &[loaded],
        ScanOptions {
            analysis_height,
            keep_traces: true,
            cancel: Some(&CANCEL),
            progress: Some(progress),
            content: Some(crop),
        },
    )?;
    let trace = res.traces.into_iter().next();
    Ok(TestResult {
        events: res.events,
        hits: trace
            .as_ref()
            .map(|t| {
                t.hits
                    .iter()
                    .map(|(t_ms, location)| TestHit {
                        t_ms: *t_ms,
                        location: *location,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        samples: trace.map(|t| t.samples).unwrap_or_default(),
        duration_ms: info.duration_ms,
        cancelled: res.cancelled,
    })
}

// ---- Screen readers (match details) ----

/// A labelled crop for a choice reader option: cut `region` from the frame as a reference for `value`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionCrop {
    pub value: String,
    pub region: Region,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub field: String,
    pub value: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameReadResult {
    /// Slot that was read (None when the highlight colour was not found).
    pub slot: Option<u32>,
    pub readings: Vec<Reading>,
    /// False when text recognition is unavailable on this computer.
    pub ocr_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderTestResult {
    pub details: recap_core::details::MatchDetails,
    pub readings: Vec<(u64, String, String)>,
    pub frames: u32,
    pub window: (u64, u64),
}

/// Where a reader's reference images live: user folder first, then the loaded game folders.
fn reader_base(st: &AppState, game_id: &str, r: &ReaderDef) -> PathBuf {
    let files: Vec<&String> = r.options.iter().flat_map(|o| o.templates.iter()).collect();
    let mut candidates = vec![st.paths.user_games.join(game_id)];
    if let Some(g) = st.game(game_id) {
        candidates.push(g.dir.clone());
        candidates.extend(g.readers.iter().map(|x| x.base_dir.clone()));
    }
    candidates.push(st.paths.bundled_games.join(game_id));
    candidates
        .iter()
        .find(|c| files.iter().all(|t| c.join(t).exists()))
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

/// Save a reader into the user's game folder, cutting any labelled crops from the frame.
pub fn save_reader(st: &AppState, game_id: &str, mut r: ReaderDef, source: Option<&FrameSource>, crops: &[OptionCrop]) -> Result<()> {
    let game = st.game(game_id).context("unknown game")?;
    let user_dir = st.paths.user_games.join(game_id);
    // References that live in the bundled folder must be copied next to the user's reader.
    let base = reader_base(st, game_id, &r);
    if base != user_dir {
        for t in r.options.iter().flat_map(|o| o.templates.iter()) {
            let (src, dst) = (base.join(t), user_dir.join(t));
            if src.exists() && !dst.exists() {
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(&src, &dst)?;
            }
        }
    }
    let mut new_files = Vec::new();
    if !crops.is_empty() {
        let img = load_image(st, source.context("load a frame before adding a reference")?)?;
        let has_refs = r.options.iter().any(|o| !o.templates.is_empty());
        if !has_refs {
            r.template_frame_height = img.height();
        }
        for (i, c) in crops.iter().enumerate() {
            let value = c.value.trim();
            if value.is_empty() {
                bail!("give the reference a value (e.g. the agent's name)");
            }
            c.region.validate()?;
            let (x, y, w, h) = c.region.to_pixels(img.width(), img.height());
            if w < 6 || h < 6 {
                bail!("the box for {value} is too small; drag a larger rectangle");
            }
            let mut crop = img.crop_imm(x, y, w, h);
            if has_refs && r.template_frame_height != img.height() {
                let f = r.template_frame_height as f32 / img.height() as f32;
                crop = crop.resize_exact(
                    ((w as f32 * f).round() as u32).max(4),
                    ((h as f32 * f).round() as u32).max(4),
                    image::imageops::FilterType::Triangle,
                );
            }
            let mut png = Vec::new();
            crop.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)?;
            let name = format!(
                "{}-{}-{}-{i}.png",
                r.id,
                recap_core::events::slugify(value).replace('_', "-"),
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            );
            let rel = format!("templates/{name}");
            match r.options.iter_mut().find(|o| o.value.eq_ignore_ascii_case(value)) {
                Some(o) => o.templates.push(rel),
                None => r.options.push(ChoiceOption {
                    value: value.to_string(),
                    templates: vec![rel],
                }),
            }
            new_files.push((name, png));
        }
    }
    if r.label.trim().is_empty() {
        r.label = r.id.replace(['-', '_'], " ");
    }
    save_user_reader(&st.paths.user_games, &game.def, &r, &new_files)?;
    Ok(())
}

fn loaded_reader(st: &AppState, game_id: &str, r: ReaderDef) -> Result<LoadedReader> {
    r.validate()?;
    let base_dir = reader_base(st, game_id, &r);
    let mut def = r;
    def.enabled = true;
    Ok(LoadedReader { def, base_dir, user: true })
}

/// Read a single frame right away (Studio's live preview).
pub fn read_frame(st: &AppState, game_id: &str, r: ReaderDef, source: &FrameSource) -> Result<FrameReadResult> {
    let loaded = loaded_reader(st, game_id, r)?;
    let img = load_image(st, source)?.to_rgb8();
    let p = PreparedReader::load(&loaded, img.width(), img.height())?;
    let ocr = crate::ocr::text_reader();
    let readings = p
        .read_frame(&img, ocr.as_deref())
        .into_iter()
        .map(|(field, value, confidence)| Reading { field, value, confidence })
        .collect();
    Ok(FrameReadResult {
        slot: p.pick_slot(&img),
        readings,
        ocr_available: ocr.is_some(),
    })
}

/// Run a reader over a recording the way processing does, and show every reading.
pub fn test_reader(st: &AppState, game_id: &str, r: ReaderDef, m: &recap_core::library::MatchRecord) -> Result<ReaderTestResult> {
    let loaded = loaded_reader(st, game_id, r)?;
    let when = loaded.def.when;
    let events = st.library.lock().unwrap().list_events(&m.id)?;
    let t = |k: &str, first: bool| {
        let it = events.iter().filter(|e| e.kind == k).map(|e| e.t_ms);
        if first {
            it.min()
        } else {
            it.max()
        }
    };
    let window = ReadWindows::for_match(
        m.duration_ms,
        t(recap_core::events::kind::MATCH_START, true),
        t(recap_core::events::kind::MATCH_END, false),
        None,
    );
    let ocr = crate::ocr::text_reader();
    let mut traces = Vec::new();
    let details = read_details(&st.ff, Path::new(&m.path), &[loaded], &window, ocr.as_deref(), Some(&mut traces))?;
    let tr = traces.into_iter().next();
    Ok(ReaderTestResult {
        details,
        readings: tr.as_ref().map(|t| t.readings.clone()).unwrap_or_default(),
        frames: tr.map(|t| t.frames).unwrap_or(0),
        window: match when {
            recap_core::games::ReadWhen::Start => window.start,
            recap_core::games::ReadWhen::End => window.end,
            recap_core::games::ReadWhen::Any => window.any,
        },
    })
}
