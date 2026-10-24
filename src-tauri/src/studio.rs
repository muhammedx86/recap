//! Detector Studio backend: load frames, cut templates, test detectors.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use image::{DynamicImage, ImageFormat};
use recap_core::{
    events::GameEvent,
    games::{ensure_templates_copied, save_user_detector, DetectorDef, LoadedDetector, Region},
    scan::{scan_video, ScanOptions},
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
pub struct TestResult {
    pub events: Vec<GameEvent>,
    pub samples: Vec<(u64, f32)>,
    pub duration_ms: u64,
}

fn load_image(st: &AppState, source: &FrameSource) -> Result<DynamicImage> {
    match source {
        FrameSource::Video { path, t_ms } => {
            // Show (and cut templates from) the game picture only, so boxes line up with scans.
            let video = Path::new(path);
            let crop = st.ff.probe(video).ok().and_then(|info| st.ff.content_rect(video, &info));
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
    let info = st.ff.probe(match_path)?;
    let analysis_height = st.settings.read().unwrap().analysis_height;
    let res = scan_video(
        &st.ff,
        match_path,
        &[loaded],
        ScanOptions {
            analysis_height,
            keep_traces: true,
            cancel: None,
            progress: None,
        },
    )?;
    Ok(TestResult {
        events: res.events,
        samples: res.traces.into_iter().next().map(|t| t.samples).unwrap_or_default(),
        duration_ms: info.duration_ms,
    })
}
