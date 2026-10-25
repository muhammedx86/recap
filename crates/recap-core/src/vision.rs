//! Template matching for vision detectors.
//!
//! Each detector looks only at its own small region of the frame. Inside that
//! region it slides each reference image and computes zero-mean normalized
//! cross-correlation (ZNCC), which is insensitive to brightness and contrast
//! changes. Work is bounded by downscaling so the template is at most
//! [`MAX_TEMPLATE_DIM`] pixels on its longest side.

use std::path::Path;

use anyhow::{Context, Result};
use image::{imageops::FilterType, GrayImage, RgbImage};
use serde::Serialize;

use crate::{
    events::{EventSource, GameEvent},
    games::{hud_scale, DetectMethod, DetectorDef, LoadedDetector, Region, ValueFire},
};

/// A decoded analysis frame: grayscale normally, colour when a detector filters by colour.
#[derive(Debug, Clone, Copy)]
pub enum FrameRef<'a> {
    Gray(&'a GrayImage),
    Rgb(&'a RgbImage),
}

impl FrameRef<'_> {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            FrameRef::Gray(g) => g.dimensions(),
            FrameRef::Rgb(c) => c.dimensions(),
        }
    }
}

/// Longest side of a template after downscaling for matching.
pub const MAX_TEMPLATE_DIM: u32 = 96;

/// Pixels of two references that differ by less than this (0..255) count as part of the
/// on-screen element; the rest is game scene showing through.
const MASK_AGREE: i32 = 24;
/// Max shift (reference pixels) searched when lining up two references of the same element.
const MASK_ALIGN: i32 = 6;
/// Two references count as the same element when they correlate at least this well.
const MASK_SAME_ELEMENT: f32 = 0.3;
/// Windows whose pixel variance (0..255 scale) is below this are flat (no element in them)
/// and never match: about 1.5 grey levels of spread.
const MIN_WINDOW_VAR: f64 = 2.25;

/// Template pixels that belong to the HUD element (the rest are ignored).
#[derive(Debug, Clone)]
struct MaskedPixels {
    /// (x, y) of each kept pixel.
    pos: Vec<(u32, u32)>,
    /// Kept pixel values, zero mean and unit norm over the kept pixels.
    zvals: Vec<f32>,
}

/// A reference image prepared for a specific analysis frame height.
#[derive(Debug, Clone)]
struct PreparedTemplate {
    img: GrayImage,
    /// Pixel values with the mean subtracted.
    zero_mean: Vec<f32>,
    /// sqrt(sum((t - mean)^2))
    norm: f32,
    /// When set, only these pixels are compared.
    mask: Option<MaskedPixels>,
    /// Which reference image and scale this came from (for "where did it match").
    reference: usize,
    scale: f32,
}

impl PreparedTemplate {
    /// Like `new`, restricted to the pixels where `mask` is non-zero (same size as `img`).
    fn with_mask(img: GrayImage, mask: Option<&GrayImage>) -> Option<Self> {
        let mut t = Self::new(img)?;
        if let Some(m) = mask {
            let pos: Vec<(u32, u32)> = m
                .enumerate_pixels()
                .filter(|(_, _, v)| v.0[0] > 127)
                .map(|(x, y, _)| (x, y))
                .collect();
            if pos.len() >= 16 {
                let vals: Vec<f32> = pos.iter().map(|&(x, y)| t.img.get_pixel(x, y).0[0] as f32).collect();
                let mean = vals.iter().sum::<f32>() / vals.len() as f32;
                let norm = vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>().sqrt();
                if norm > 1e-3 {
                    let zvals = vals.iter().map(|v| (v - mean) / norm).collect();
                    t.mask = Some(MaskedPixels { pos, zvals });
                }
            }
        }
        Some(t)
    }

    fn new(img: GrayImage) -> Option<Self> {
        let n = (img.width() * img.height()) as f32;
        if n < 4.0 {
            return None;
        }
        let mean = img.as_raw().iter().map(|&v| v as f32).sum::<f32>() / n;
        let zero_mean: Vec<f32> = img.as_raw().iter().map(|&v| v as f32 - mean).collect();
        let norm = zero_mean.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm < 1e-3 {
            // A flat template matches everything; refuse it.
            return None;
        }
        Some(Self {
            img,
            zero_mean,
            norm,
            mask: None,
            reference: 0,
            scale: 1.0,
        })
    }
}

/// ZNCC of `a` against `b` shifted by (dx, dy), over their overlap.
fn shifted_zncc(a: &GrayImage, b: &GrayImage, dx: i32, dy: i32) -> f32 {
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for (x, y, p) in a.enumerate_pixels() {
        let (bx, by) = (x as i32 + dx, y as i32 + dy);
        if bx < 0 || by < 0 || bx >= b.width() as i32 || by >= b.height() as i32 {
            continue;
        }
        let va = p.0[0] as f64;
        let vb = b.get_pixel(bx as u32, by as u32).0[0] as f64;
        sa += va;
        sb += vb;
        saa += va * va;
        sbb += vb * vb;
        sab += va * vb;
        n += 1.0;
    }
    if n < 16.0 {
        return -1.0;
    }
    let cov = sab - sa * sb / n;
    let den = ((saa - sa * sa / n) * (sbb - sb * sb / n)).sqrt();
    if den < 1e-6 {
        -1.0
    } else {
        (cov / den) as f32
    }
}

/// Learn which pixels of reference `i` are the HUD element: those that agree with every other
/// reference of the same element (lined up first). Returns None when there is nothing to learn
/// from (a single reference, or the others show something else) or the result looks unreliable.
pub fn reference_mask(imgs: &[GrayImage], i: usize) -> Option<GrayImage> {
    let a = &imgs[i];
    let mut agree = vec![true; (a.width() * a.height()) as usize];
    let mut others = 0;
    for (j, b) in imgs.iter().enumerate() {
        if j == i {
            continue;
        }
        let mut best = (-1.0f32, 0, 0);
        // Search shifts around the centre-aligned position (references may differ slightly in size).
        let (cx, cy) = (
            (b.width() as i32 - a.width() as i32) / 2,
            (b.height() as i32 - a.height() as i32) / 2,
        );
        for dy in -MASK_ALIGN..=MASK_ALIGN {
            for dx in -MASK_ALIGN..=MASK_ALIGN {
                let s = shifted_zncc(a, b, cx + dx, cy + dy);
                if s > best.0 {
                    best = (s, cx + dx, cy + dy);
                }
            }
        }
        if best.0 < MASK_SAME_ELEMENT {
            continue;
        }
        others += 1;
        let (dx, dy) = (best.1, best.2);
        for (x, y, p) in a.enumerate_pixels() {
            let (bx, by) = (x as i32 + dx, y as i32 + dy);
            if bx < 0 || by < 0 || bx >= b.width() as i32 || by >= b.height() as i32 {
                continue;
            }
            let d = (p.0[0] as i32 - b.get_pixel(bx as u32, by as u32).0[0] as i32).abs();
            if d >= MASK_AGREE {
                agree[(y * a.width() + x) as usize] = false;
            }
        }
    }
    if others == 0 {
        return None;
    }
    let kept = agree.iter().filter(|&&k| k).count() as f32 / agree.len() as f32;
    if !(0.08..=0.97).contains(&kept) {
        return None;
    }
    Some(GrayImage::from_fn(a.width(), a.height(), |x, y| {
        image::Luma([if agree[(y * a.width() + x) as usize] { 255 } else { 0 }])
    }))
}

/// Best masked ZNCC of `t` anywhere inside `hay`, with its top-left position.
fn best_masked_zncc(hay: &GrayImage, t: &PreparedTemplate, m: &MaskedPixels) -> (f32, u32, u32) {
    let (hw, hh) = hay.dimensions();
    let (tw, th) = t.img.dimensions();
    if tw > hw || th > hh {
        return (-1.0, 0, 0);
    }
    let raw = hay.as_raw();
    let offs: Vec<usize> = m.pos.iter().map(|&(x, y)| (y * hw + x) as usize).collect();
    let n = offs.len() as f64;
    let mut best = (-1f32, 0, 0);
    for y in 0..=(hh - th) as usize {
        for x in 0..=(hw - tw) as usize {
            let base = y * hw as usize + x;
            // f64 sums: in f32, sum(v^2) - sum(v)^2/n cancels catastrophically on flat bright
            // areas and turns noise into perfect "matches".
            let (mut s, mut q, mut cross) = (0f64, 0f64, 0f64);
            for (o, tv) in offs.iter().zip(&m.zvals) {
                let v = raw[base + o] as f64;
                s += v;
                q += v * v;
                cross += v * *tv as f64;
            }
            let var = q - s * s / n;
            // A (nearly) flat window has no element in it, whatever the correlation says.
            if var <= MIN_WINDOW_VAR * n {
                continue;
            }
            // zvals are zero mean, so the window mean cancels out of `cross`.
            let score = (cross / var.sqrt()) as f32;
            if score > best.0 {
                best = (score, x as u32, y as u32);
            }
        }
    }
    (best.0.min(1.0), best.1, best.2)
}

/// Best ZNCC score of `t` anywhere inside `hay`. Returns -1..1 (1 = identical up to brightness/contrast).
fn best_zncc(hay: &GrayImage, t: &PreparedTemplate) -> f32 {
    best_zncc_at(hay, t).0
}

/// [`best_zncc`] with the top-left position of the best match.
fn best_zncc_at(hay: &GrayImage, t: &PreparedTemplate) -> (f32, u32, u32) {
    if let Some(m) = &t.mask {
        return best_masked_zncc(hay, t, m);
    }
    let (hw, hh) = hay.dimensions();
    let (tw, th) = t.img.dimensions();
    if tw > hw || th > hh {
        return (-1.0, 0, 0);
    }
    let n = (tw * th) as f64;
    // Integral images of the haystack for O(1) window sums.
    let stride = (hw + 1) as usize;
    let mut sum = vec![0f64; stride * (hh as usize + 1)];
    let mut sq = vec![0f64; stride * (hh as usize + 1)];
    let raw = hay.as_raw();
    for y in 0..hh as usize {
        let mut rs = 0f64;
        let mut rq = 0f64;
        for x in 0..hw as usize {
            let v = raw[y * hw as usize + x] as f64;
            rs += v;
            rq += v * v;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + rs;
            sq[(y + 1) * stride + x + 1] = sq[y * stride + x + 1] + rq;
        }
    }
    let rect = |a: &Vec<f64>, x: usize, y: usize| {
        a[(y + th as usize) * stride + x + tw as usize] - a[y * stride + x + tw as usize] - a[(y + th as usize) * stride + x]
            + a[y * stride + x]
    };
    let mut best = (-1f32, 0, 0);
    for y in 0..=(hh - th) as usize {
        for x in 0..=(hw - tw) as usize {
            let s = rect(&sum, x, y);
            let q = rect(&sq, x, y);
            let var = q - s * s / n;
            if var <= MIN_WINDOW_VAR * n {
                continue;
            }
            // sum(T' * I) — T' is zero mean, so the window mean cancels out.
            let mut cross = 0f32;
            for ty in 0..th as usize {
                let row = (y + ty) * hw as usize + x;
                let trow = ty * tw as usize;
                let hrow = &raw[row..row + tw as usize];
                let trowv = &t.zero_mean[trow..trow + tw as usize];
                for (h, tv) in hrow.iter().zip(trowv) {
                    cross += *h as f32 * tv;
                }
            }
            let score = cross / (t.norm * (var.sqrt() as f32));
            if score > best.0 {
                best = (score, x as u32, y as u32);
            }
        }
    }
    (best.0.min(1.0), best.1, best.2)
}

/// Where a detector matched best on a frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchLocation {
    pub score: f32,
    /// The matched box, as fractions of the frame.
    pub region: Region,
    /// Template scale that matched (1.0 = the size it was cut at).
    pub scale: f32,
    /// Index of the reference image that matched best.
    pub reference: usize,
}

/// A detector ready to score frames of a known size.
#[derive(Debug, Clone)]
pub struct PreparedDetector {
    pub def: DetectorDef,
    /// One entry per (template, scale).
    templates: Vec<PreparedTemplate>,
    /// Factor applied to the cropped region before matching.
    work_scale: f32,
    frame_w: u32,
    frame_h: u32,
}

impl PreparedDetector {
    /// Load the detector's templates and scale them for frames of `frame_w`x`frame_h`.
    pub fn load(det: &LoadedDetector, frame_w: u32, frame_h: u32) -> Result<Self> {
        let mut imgs = Vec::new();
        let mut given = Vec::new();
        for p in det.template_paths() {
            imgs.push(match &det.def.color_filter {
                // The references go through the same colour filter as the frames.
                Some(cf) => cf.apply(&load_rgb(&p)?),
                None => load_gray(&p)?,
            });
            given.push(mask_path(&p).filter(|m| m.exists()).and_then(|m| load_gray(&m).ok()));
        }
        Self::from_images_with_masks(det.def.clone(), imgs, given, frame_w, frame_h)
    }

    pub fn from_images(def: DetectorDef, imgs: Vec<GrayImage>, frame_w: u32, frame_h: u32) -> Result<Self> {
        let n = imgs.len();
        Self::from_images_with_masks(def, imgs, vec![None; n], frame_w, frame_h)
    }

    /// Like [`Self::from_images`], with an optional hand-made mask per reference (white = part of
    /// the HUD element). References without one get a mask learned from the others.
    pub fn from_images_with_masks(
        def: DetectorDef,
        imgs: Vec<GrayImage>,
        given: Vec<Option<GrayImage>>,
        frame_w: u32,
        frame_h: u32,
    ) -> Result<Self> {
        // Anchored HUDs scale to fit the frame (by width on screens narrower than the reference).
        let fit = if def.anchor.is_some() {
            hud_scale(def.ref_aspect(), frame_w, frame_h)
        } else {
            1.0
        };
        let base = fit * frame_h as f32 / def.template_frame_height as f32;
        let mut scales = vec![1.0f32];
        for s in &def.scales {
            if (*s - 1.0).abs() > 1e-3 && *s > 0.2 && *s < 5.0 {
                scales.push(*s);
            }
        }
        // Pick a work scale so the largest template is <= MAX_TEMPLATE_DIM.
        let largest = imgs
            .iter()
            .map(|i| i.width().max(i.height()) as f32 * base * scales.iter().cloned().fold(1.0, f32::max))
            .fold(1.0, f32::max);
        let work_scale = (MAX_TEMPLATE_DIM as f32 / largest).min(1.0);
        let masks: Vec<Option<GrayImage>> = (0..imgs.len())
            .map(|i| match given.get(i).cloned().flatten() {
                Some(m) if m.dimensions() == imgs[i].dimensions() => Some(m),
                _ => reference_mask(&imgs, i),
            })
            .collect();
        let mut templates = Vec::new();
        for (ri, (img, mask)) in imgs.iter().zip(&masks).enumerate() {
            for s in &scales {
                let f = base * s * work_scale;
                let w = ((img.width() as f32 * f).round() as u32).max(2);
                let h = ((img.height() as f32 * f).round() as u32).max(2);
                let resized = image::imageops::resize(img, w, h, FilterType::Triangle);
                let resized = if def.method == DetectMethod::Edges {
                    edges(&resized)
                } else {
                    resized
                };
                let mask = mask.as_ref().map(|m| image::imageops::resize(m, w, h, FilterType::Nearest));
                if let Some(mut t) = PreparedTemplate::with_mask(resized, mask.as_ref()) {
                    t.reference = ri;
                    t.scale = *s;
                    templates.push(t);
                }
            }
        }
        anyhow::ensure!(!templates.is_empty(), "detector {} has no usable reference images", def.id);
        Ok(Self {
            def,
            templates,
            work_scale,
            frame_w,
            frame_h,
        })
    }

    pub fn frame_size(&self) -> (u32, u32) {
        (self.frame_w, self.frame_h)
    }

    /// Whether frames must be decoded in colour for this detector.
    pub fn needs_color(&self) -> bool {
        self.def.color_filter.is_some()
    }

    /// The detector's search box on `frame`, as the grayscale (or colour-filtered) image it
    /// matches against, with its top-left corner.
    fn crop(&self, frame: FrameRef) -> (GrayImage, u32, u32) {
        let (fw, fh) = frame.dimensions();
        let (x, y, w, h) = self.def.region_for(fw, fh).to_pixels(fw, fh);
        let img = match (frame, &self.def.color_filter) {
            (FrameRef::Gray(g), _) => image::imageops::crop_imm(g, x, y, w, h).to_image(),
            (FrameRef::Rgb(c), Some(cf)) => cf.apply(&image::imageops::crop_imm(c, x, y, w, h).to_image()),
            (FrameRef::Rgb(c), None) => image::imageops::grayscale(&image::imageops::crop_imm(c, x, y, w, h).to_image()),
        };
        (img, x, y)
    }

    /// Score a full grayscale frame (must be the size given at load time).
    pub fn score_frame(&self, frame: &GrayImage) -> f32 {
        self.score(FrameRef::Gray(frame)).0
    }

    /// Score a frame: (best score, index of the reference image that scored it).
    pub fn score(&self, frame: FrameRef) -> (f32, usize) {
        let (crop, _, _) = self.crop(frame);
        self.score_region_ref(&crop)
    }

    /// Where the detector matches best on a full frame (same size as at load time).
    pub fn locate_frame(&self, frame: &GrayImage) -> Option<MatchLocation> {
        self.locate(FrameRef::Gray(frame))
    }

    /// Where the detector matches best on a frame (same size as at load time).
    pub fn locate(&self, frame: FrameRef) -> Option<MatchLocation> {
        let (fw, fh) = frame.dimensions();
        let (fw, fh) = (fw as f32, fh as f32);
        let (crop, x0, y0) = self.crop(frame);
        let hay = self.prepare_hay(&crop);
        // Best template, with its match position in the (unpadded) work-scale crop.
        let mut best: Option<(f32, f32, f32, &PreparedTemplate)> = None;
        for t in &self.templates {
            let padded = pad_to(&hay, t.img.width(), t.img.height());
            // pad_to centres the original inside the padded image.
            let off_x = ((padded.width() - hay.width()) / 2) as f32;
            let off_y = ((padded.height() - hay.height()) / 2) as f32;
            let (s, x, y) = best_zncc_at(&padded, t);
            if best.as_ref().is_none_or(|b| s > b.0) {
                best = Some((s, (x as f32 - off_x).max(0.0), (y as f32 - off_y).max(0.0), t));
            }
        }
        let (score, x, y, t) = best?;
        let ws = self.work_scale.max(1e-6);
        let px = x0 as f32 + x / ws;
        let py = y0 as f32 + y / ws;
        let (tw, th) = (t.img.width() as f32 / ws, t.img.height() as f32 / ws);
        Some(MatchLocation {
            score,
            region: Region {
                x: (px / fw).clamp(0.0, 1.0),
                y: (py / fh).clamp(0.0, 1.0),
                w: (tw / fw).min(1.0),
                h: (th / fh).min(1.0),
            },
            scale: t.scale,
            reference: t.reference,
        })
    }

    fn prepare_hay(&self, crop: &GrayImage) -> GrayImage {
        let hay = if self.work_scale < 0.999 {
            let w = ((crop.width() as f32 * self.work_scale).round() as u32).max(1);
            let h = ((crop.height() as f32 * self.work_scale).round() as u32).max(1);
            image::imageops::resize(crop, w, h, FilterType::Triangle)
        } else {
            crop.clone()
        };
        if self.def.method == DetectMethod::Edges {
            edges(&hay)
        } else {
            hay
        }
    }

    /// Score an already-cropped region (at analysis resolution).
    pub fn score_region(&self, crop: &GrayImage) -> f32 {
        self.score_region_ref(crop).0
    }

    /// [`Self::score_region`] with the index of the reference image that scored best.
    pub fn score_region_ref(&self, crop: &GrayImage) -> (f32, usize) {
        let hay = self.prepare_hay(crop);
        self.templates
            .iter()
            .map(|t| (best_zncc(&pad_to(&hay, t.img.width(), t.img.height()), t), t.reference))
            .fold((-1.0, 0), |a, b| if b.0 > a.0 { b } else { a })
    }

    /// The value of reference `i` (value detectors).
    pub fn value_of(&self, reference: usize) -> Option<&str> {
        // A reference without a value (left blank) reads as nothing.
        self.def.values.get(reference).map(|v| v.trim()).filter(|v| !v.is_empty())
    }
}

/// Extend `img` by repeating its edge pixels so it is at least `w`x`h` (rounding can leave a
/// tight search region a pixel smaller than its template).
fn pad_to(img: &GrayImage, w: u32, h: u32) -> std::borrow::Cow<'_, GrayImage> {
    if img.width() >= w && img.height() >= h {
        return std::borrow::Cow::Borrowed(img);
    }
    let (nw, nh) = (img.width().max(w), img.height().max(h));
    let (ox, oy) = ((nw - img.width()) / 2, (nh - img.height()) / 2);
    std::borrow::Cow::Owned(GrayImage::from_fn(nw, nh, |x, y| {
        let sx = x.saturating_sub(ox).min(img.width() - 1);
        let sy = y.saturating_sub(oy).min(img.height() - 1);
        *img.get_pixel(sx, sy)
    }))
}

/// Sobel edge strength, scaled to 0..255.
pub fn edges(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return img.clone();
    }
    let p = |x: u32, y: u32| img.get_pixel(x.clamp(0, w - 1), y.clamp(0, h - 1)).0[0] as i32;
    GrayImage::from_fn(w, h, |x, y| {
        let (xm, xp) = (x.saturating_sub(1), (x + 1).min(w - 1));
        let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
        let gx = p(xp, ym) + 2 * p(xp, y) + p(xp, yp) - p(xm, ym) - 2 * p(xm, y) - p(xm, yp);
        let gy = p(xm, yp) + 2 * p(x, yp) + p(xp, yp) - p(xm, ym) - 2 * p(x, ym) - p(xp, ym);
        let mag = ((gx * gx + gy * gy) as f32).sqrt() / 4.0;
        image::Luma([mag.min(255.0) as u8])
    })
}

/// `templates/ring.png` -> `templates/ring.mask.png`: an optional hand-made mask for a reference.
pub fn mask_path(template: &Path) -> Option<std::path::PathBuf> {
    let stem = template.file_stem()?.to_str()?;
    if stem.ends_with(".mask") {
        return None;
    }
    Some(template.with_file_name(format!("{stem}.mask.png")))
}

pub fn load_rgb(path: &Path) -> Result<RgbImage> {
    Ok(image::open(path)
        .with_context(|| format!("opening template {}", path.display()))?
        .to_rgb8())
}

pub fn load_gray(path: &Path) -> Result<GrayImage> {
    Ok(image::open(path)
        .with_context(|| format!("opening template {}", path.display()))?
        .to_luma8())
}

/// Turns a stream of per-sample scores into events: fires on the rising edge after
/// `min_consecutive` matching samples, never more often than `cooldown_ms`.
#[derive(Debug, Clone)]
pub struct Trigger {
    threshold: f32,
    cooldown_ms: u64,
    min_consecutive: u32,
    rearm_drop: f32,
    hold_margin: f32,
    run: u32,
    run_start_ms: u64,
    run_peak: f32,
    fired_this_run: bool,
    /// Re-arming within a run: highest score since the last fire, then the lowest score
    /// once it has dipped by `rearm_drop`.
    high: f32,
    trough: f32,
    armed: bool,
    last_fire_ms: Option<u64>,
}

impl Trigger {
    pub fn new(def: &DetectorDef) -> Self {
        Self {
            threshold: def.threshold,
            cooldown_ms: def.cooldown_ms,
            min_consecutive: def.min_consecutive.max(1),
            rearm_drop: def.rearm_drop.max(0.0),
            hold_margin: def.hold_margin.clamp(0.0, 1.0),
            run: 0,
            run_start_ms: 0,
            run_peak: 0.0,
            fired_this_run: false,
            high: 0.0,
            trough: 0.0,
            armed: false,
            last_fire_ms: None,
        }
    }

    /// Feed one sample. Returns `Some((event_time_ms, peak_score))` when the detector fires.
    pub fn push(&mut self, t_ms: u64, score: f32) -> Option<(u64, f32)> {
        // After firing, stay "on screen" until the score drops `hold_margin` below the threshold.
        let held = self.run > 0 && self.fired_this_run && score >= self.threshold - self.hold_margin;
        if score >= self.threshold || held {
            if self.run == 0 {
                self.run_start_ms = t_ms;
                self.run_peak = score;
                self.fired_this_run = false;
            }
            self.run += 1;
            self.run_peak = self.run_peak.max(score);
            // Still on screen but it dipped and recovered: a new occurrence (e.g. another kill).
            if self.fired_this_run && self.rearm_drop > 0.0 {
                if !self.armed {
                    self.high = self.high.max(score);
                    if self.high - score >= self.rearm_drop {
                        self.armed = true;
                        self.trough = score;
                    }
                } else {
                    self.trough = self.trough.min(score);
                    let cooled = self.last_fire_ms.is_none_or(|l| t_ms.saturating_sub(l) >= self.cooldown_ms);
                    if score - self.trough >= self.rearm_drop && cooled {
                        self.last_fire_ms = Some(t_ms);
                        self.rearm(score);
                        return Some((t_ms, score));
                    }
                }
            }
            if !self.fired_this_run && self.run >= self.min_consecutive {
                let cooled = self
                    .last_fire_ms
                    .is_none_or(|l| self.run_start_ms.saturating_sub(l) >= self.cooldown_ms);
                self.fired_this_run = true;
                self.rearm(self.run_peak);
                if cooled {
                    self.last_fire_ms = Some(self.run_start_ms);
                    return Some((self.run_start_ms, self.run_peak));
                }
            }
        } else {
            self.run = 0;
        }
        None
    }

    fn rearm(&mut self, score: f32) {
        self.high = score;
        self.trough = score;
        self.armed = false;
    }
}

/// Settles a value detector's readings by voting and reports changes.
#[derive(Debug, Clone)]
pub struct ValueTrigger {
    threshold: f32,
    votes: usize,
    window: usize,
    fire: ValueFire,
    recent: std::collections::VecDeque<Option<String>>,
    settled: Option<String>,
}

/// One change of a value detector's settled value.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange {
    pub t_ms: u64,
    pub value: String,
    pub previous: String,
    /// Events to emit for this change (more than one when a counter jumps by several steps).
    pub count: u32,
}

impl ValueTrigger {
    pub fn new(def: &DetectorDef) -> Self {
        let rule = def.value_rule.clone().unwrap_or(crate::games::ValueRule {
            votes: 3,
            window: 5,
            fire: ValueFire::Change,
        });
        Self {
            threshold: def.threshold,
            votes: rule.votes.max(1) as usize,
            window: rule.window.max(rule.votes).max(1) as usize,
            fire: rule.fire,
            recent: Default::default(),
            settled: None,
        }
    }

    /// Feed one sample: the best score and the value of the reference that produced it.
    pub fn push(&mut self, t_ms: u64, score: f32, value: Option<&str>) -> Option<ValueChange> {
        let reading = (score >= self.threshold).then(|| value.map(str::to_string)).flatten();
        self.recent.push_back(reading);
        while self.recent.len() > self.window {
            self.recent.pop_front();
        }
        // The value most of the window agrees on, if enough samples do.
        let mut counts: Vec<(&String, usize)> = Vec::new();
        for v in self.recent.iter().flatten() {
            match counts.iter_mut().find(|(c, _)| *c == v) {
                Some(e) => e.1 += 1,
                None => counts.push((v, 1)),
            }
        }
        let (winner, n) = counts.into_iter().max_by_key(|(_, n)| *n)?;
        if n < self.votes || self.settled.as_ref() == Some(winner) {
            return None;
        }
        let winner = winner.clone();
        let previous = self.settled.replace(winner.clone());
        let previous = previous?; // the first settled value is only a baseline
        let count = match self.fire {
            ValueFire::Change => 1,
            ValueFire::Increase => match (winner.trim().parse::<i64>(), previous.trim().parse::<i64>()) {
                (Ok(a), Ok(b)) if a > b => ((a - b) as u32).min(5),
                _ => 0,
            },
        };
        (count > 0).then_some(ValueChange {
            t_ms,
            value: winner,
            previous,
            count,
        })
    }
}

/// Convenience: run a detector over an iterator of (t_ms, frame) and collect events.
pub fn detect_events<'a>(det: &PreparedDetector, frames: impl IntoIterator<Item = (u64, &'a GrayImage)>) -> Vec<GameEvent> {
    let mut trig = Trigger::new(&det.def);
    let mut out = Vec::new();
    for (t, f) in frames {
        let s = det.score_frame(f);
        if let Some((at, peak)) = trig.push(t, s) {
            out.push(
                GameEvent::new(
                    det.def.event.clone(),
                    at,
                    EventSource::Vision {
                        detector: det.def.id.clone(),
                    },
                )
                .with_confidence(peak),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::{DetectMethod, Region};
    use image::Luma;

    fn def(region: Region) -> DetectorDef {
        DetectorDef {
            id: "k".into(),
            label: String::new(),
            event: "kill".into(),
            region,
            template_box: None,
            templates: vec![],
            template_frame_height: 360,
            method: DetectMethod::Ncc,
            threshold: 0.85,
            cooldown_ms: 1000,
            min_consecutive: 2,
            sample_fps: 5.0,
            scales: vec![],
            rearm_drop: 0.0,
            hold_margin: 0.0,
            anchor: None,
            anchor_aspect: None,
            color_filter: None,
            values: vec![],
            value_rule: None,
            blocks: vec![],
            block_margin_ms: 1000,
            enabled: true,
            notes: None,
        }
    }

    /// A distinctive pattern (a ring with a bar) to paste into frames.
    fn pattern(w: u32, h: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let cx = w as f32 / 2.0;
            let cy = h as f32 / 2.0;
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let ring = (d - w.min(h) as f32 * 0.35).abs() < 2.5;
            let bar = y > h * 3 / 4 && x > w / 4 && x < w * 3 / 4;
            Luma([if ring || bar { 230 } else { 30 }])
        })
    }

    fn noise_frame(w: u32, h: u32, seed: u32) -> GrayImage {
        let mut s = seed.wrapping_mul(2654435761).max(1);
        GrayImage::from_fn(w, h, |_, _| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            Luma([(s % 120) as u8 + 40])
        })
    }

    fn paste(frame: &mut GrayImage, p: &GrayImage, x: u32, y: u32, gain: f32, offset: f32) {
        for (px, py, v) in p.enumerate_pixels() {
            let val = (v.0[0] as f32 * gain + offset).clamp(0.0, 255.0) as u8;
            frame.put_pixel(x + px, y + py, Luma([val]));
        }
    }

    #[test]
    fn finds_template_despite_brightness_change() {
        let tpl = pattern(40, 24);
        let mut frame = noise_frame(640, 360, 7);
        // Template appears at a slightly different spot inside the region and dimmer.
        paste(&mut frame, &tpl, 300, 250, 0.6, 20.0);
        let region = Region {
            x: 260.0 / 640.0,
            y: 230.0 / 360.0,
            w: 120.0 / 640.0,
            h: 70.0 / 360.0,
        };
        let d = PreparedDetector::from_images(def(region), vec![tpl.clone()], 640, 360).unwrap();
        let hit = d.score_frame(&frame);
        let miss = d.score_frame(&noise_frame(640, 360, 9));
        assert!(hit > 0.95, "hit score {hit}");
        assert!(miss < 0.5, "miss score {miss}");
    }

    #[test]
    fn locate_reports_where_it_matched() {
        let tpl = pattern(40, 24);
        let mut frame = noise_frame(640, 360, 7);
        paste(&mut frame, &tpl, 300, 250, 1.0, 0.0);
        let region = Region {
            x: 260.0 / 640.0,
            y: 230.0 / 360.0,
            w: 120.0 / 640.0,
            h: 70.0 / 360.0,
        };
        let d = PreparedDetector::from_images(def(region), vec![tpl], 640, 360).unwrap();
        let at = d.locate_frame(&frame).unwrap();
        assert!(at.score > 0.95, "score {}", at.score);
        assert!((at.region.x * 640.0 - 300.0).abs() <= 2.0, "x {}", at.region.x * 640.0);
        assert!((at.region.y * 360.0 - 250.0).abs() <= 2.0, "y {}", at.region.y * 360.0);
        assert!((at.region.w * 640.0 - 40.0).abs() <= 2.0);
        assert_eq!(at.reference, 0);
    }

    #[test]
    fn rescales_templates_for_other_resolutions() {
        // Template cut from a 360p frame, scanned at 720p.
        let tpl = pattern(40, 24);
        let big_tpl = image::imageops::resize(&tpl, 80, 48, FilterType::Triangle);
        let mut frame = noise_frame(1280, 720, 3);
        paste(&mut frame, &big_tpl, 600, 500, 1.0, 0.0);
        let region = Region {
            x: 560.0 / 1280.0,
            y: 470.0 / 720.0,
            w: 180.0 / 1280.0,
            h: 110.0 / 720.0,
        };
        let d = PreparedDetector::from_images(def(region), vec![tpl], 1280, 720).unwrap();
        assert!(d.score_frame(&frame) > 0.9);
    }

    #[test]
    fn trigger_debounces_and_cools_down() {
        let d = def(Region {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        let mut t = Trigger::new(&d);
        let samples = [
            (0, 0.2),
            (200, 0.9),  // run starts
            (400, 0.95), // fires at 200
            (600, 0.9),  // same run, no refire
            (800, 0.1),
            (1000, 0.9), // new run within cooldown of 200? 1000-200 = 800 < 1000 -> suppressed
            (1200, 0.9),
            (1400, 0.1),
            (2000, 0.9),
            (2200, 0.88), // fires at 2000
            (2400, 0.3),
            (2600, 0.9), // single sample spike: not enough consecutive
            (2800, 0.2),
        ];
        let fired: Vec<_> = samples.iter().filter_map(|&(ms, s)| t.push(ms, s)).map(|(ms, _)| ms).collect();
        assert_eq!(fired, vec![200, 2000]);
    }

    #[test]
    fn hold_margin_ignores_flicker_around_the_threshold() {
        let mut d = def(Region {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        d.threshold = 0.6;
        d.min_consecutive = 1;
        d.cooldown_ms = 100;
        let samples = [(0, 0.7), (125, 0.52), (250, 0.8), (375, 0.3), (500, 0.75)];
        let run = |d: &DetectorDef| {
            let mut t = Trigger::new(d);
            samples
                .iter()
                .filter_map(|&(ms, s)| t.push(ms, s))
                .map(|(ms, _)| ms)
                .collect::<Vec<_>>()
        };
        assert_eq!(run(&d), vec![0, 250, 500]);
        d.hold_margin = 0.15;
        // 0.52 stays within the margin (same banner); 0.3 really left the screen.
        assert_eq!(run(&d), vec![0, 500]);
    }

    #[test]
    fn rearm_refires_on_dip_and_recover_only() {
        let mut d = def(Region {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        d.threshold = 0.7;
        d.cooldown_ms = 300;
        d.rearm_drop = 0.1;
        let mut t = Trigger::new(&d);
        let samples = [
            (0, 0.75),
            (100, 0.80), // fires at 0
            (200, 0.95), // rising alone is not a new event
            (300, 0.80), // dipped 0.15 below the high: armed
            (400, 0.78),
            (500, 0.90), // recovered 0.12 above the trough: fires
            (600, 0.85),
            (700, 0.95), // no dip since: nothing
        ];
        let fired: Vec<_> = samples.iter().filter_map(|&(ms, s)| t.push(ms, s)).map(|(ms, _)| ms).collect();
        assert_eq!(fired, vec![0, 500]);

        d.rearm_drop = 0.0;
        let mut t = Trigger::new(&d);
        assert_eq!(samples.iter().filter_map(|&(ms, s)| t.push(ms, s)).count(), 1);
    }

    #[test]
    fn edges_match_over_a_different_background() {
        // The same outline drawn over bright snow vs. dark ground, with inverted fill.
        let tpl = pattern(40, 24);
        let mut frame = GrayImage::from_pixel(640, 360, Luma([200]));
        for (x, y, v) in tpl.enumerate_pixels() {
            frame.put_pixel(300 + x, 250 + y, Luma([255 - v.0[0] / 2]));
        }
        let region = Region {
            x: 260.0 / 640.0,
            y: 230.0 / 360.0,
            w: 120.0 / 640.0,
            h: 70.0 / 360.0,
        };
        let mut dd = def(region);
        dd.method = DetectMethod::Edges;
        let d = PreparedDetector::from_images(dd, vec![tpl], 640, 360).unwrap();
        let hit = d.score_frame(&frame);
        let miss = d.score_frame(&noise_frame(640, 360, 11));
        assert!(hit > 0.85, "edges hit {hit}");
        assert!(miss < 0.5, "edges miss {miss}");
    }

    #[test]
    fn masks_learned_from_references_ignore_the_background() {
        // The same element over two different backgrounds: only the element pixels agree.
        let elem = pattern(40, 24);
        let over = |seed: u32| {
            let mut bg = noise_frame(40, 24, seed);
            for (x, y, v) in elem.enumerate_pixels() {
                if v.0[0] > 128 {
                    bg.put_pixel(x, y, *v);
                }
            }
            bg
        };
        let (r1, r2, r3) = (over(1), over(2), over(3));
        let m = reference_mask(&[r1.clone(), r2.clone(), r3.clone()], 0).expect("mask");
        // Every element pixel is kept; most of the (random) background is dropped.
        let elem_px: Vec<(u32, u32)> = elem.enumerate_pixels().filter(|p| p.2 .0[0] > 128).map(|p| (p.0, p.1)).collect();
        assert!(elem_px.iter().all(|&(x, y)| m.get_pixel(x, y).0[0] > 0));
        let kept = m.pixels().filter(|p| p.0[0] > 0).count();
        assert!(kept < elem_px.len() + (40 * 24 - elem_px.len()) / 3, "mask keeps {kept} px");

        // A third scene: the element plus a busy background. Masked matching scores it high.
        let mut frame = noise_frame(640, 360, 99);
        for (x, y, v) in elem.enumerate_pixels() {
            if v.0[0] > 128 {
                frame.put_pixel(300 + x, 250 + y, *v);
            }
        }
        let region = Region {
            x: 280.0 / 640.0,
            y: 235.0 / 360.0,
            w: 80.0 / 640.0,
            h: 55.0 / 360.0,
        };
        let masked = PreparedDetector::from_images(def(region), vec![r1.clone(), r2, r3], 640, 360).unwrap();
        let single = PreparedDetector::from_images(def(region), vec![r1], 640, 360).unwrap();
        let (hm, hs) = (masked.score_frame(&frame), single.score_frame(&frame));
        assert!(hm > 0.9 && hm > hs + 0.1, "masked {hm} vs single {hs}");
        assert!(masked.score_frame(&noise_frame(640, 360, 5)) < 0.5);

        // Unrelated references are not masked against each other.
        assert!(reference_mask(&[pattern(40, 24), noise_frame(40, 24, 3)], 0).is_none());
    }

    #[test]
    fn tight_region_still_matches() {
        // Region exactly the template size at a resolution where rounding shrinks it.
        let tpl = pattern(41, 25);
        let mut frame = noise_frame(641, 361, 5);
        paste(&mut frame, &tpl, 300, 250, 1.0, 0.0);
        let region = Region {
            x: 300.0 / 641.0,
            y: 250.0 / 361.0,
            w: 41.0 / 641.0,
            h: 25.0 / 361.0,
        };
        let mut d = def(region);
        d.template_frame_height = 361;
        let det = PreparedDetector::from_images(d, vec![tpl], 320, 180).unwrap();
        let small = image::imageops::resize(&frame, 320, 180, FilterType::Triangle);
        assert!(det.score_frame(&small) > 0.5);
    }

    #[test]
    fn flat_template_is_rejected() {
        let flat = GrayImage::from_pixel(20, 20, Luma([128]));
        assert!(PreparedDetector::from_images(
            def(Region {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0
            }),
            vec![flat],
            100,
            100
        )
        .is_err());
    }

    #[test]
    fn value_trigger_votes_and_counts_steps() {
        let mut d = def(Region {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        d.threshold = 0.7;
        d.values = vec!["0".into(), "1".into(), "2".into(), "3".into()];
        d.value_rule = Some(crate::games::ValueRule {
            votes: 2,
            window: 3,
            fire: ValueFire::Increase,
        });
        let mut v = ValueTrigger::new(&d);
        let feed = [
            (0, 0.9, "0"),
            (100, 0.9, "0"), // baseline 0: no event
            (200, 0.9, "8"), // one stray reading: no vote
            (300, 0.5, "1"), // below the threshold: ignored
            (400, 0.9, "1"),
            (500, 0.9, "1"), // settles on 1: one kill
            (600, 0.9, "3"),
            (700, 0.9, "3"), // 1 -> 3: two kills
            (800, 0.9, "2"),
            (900, 0.9, "2"), // a decrease never fires on "increase"
        ];
        let got: Vec<(u64, String, u32)> = feed
            .iter()
            .filter_map(|&(t, s, val)| v.push(t, s, Some(val)))
            .map(|c| (c.t_ms, c.value, c.count))
            .collect();
        assert_eq!(got, vec![(500, "1".into(), 1), (700, "3".into(), 2)]);
    }

    #[test]
    fn anchored_detector_finds_the_element_on_an_ultrawide_frame() {
        // The element sits right of the bottom centre of a 16:9 frame; on a 2.4:1 frame of the
        // same height it keeps its offset from the centre, at the same pixel size.
        let tpl = pattern(40, 24);
        let mut d = def(Region {
            x: (560.0 - 40.0) / 640.0,
            y: (300.0 - 10.0) / 360.0,
            w: 80.0 / 640.0,
            h: 44.0 / 360.0,
        });
        d.anchor = Some(crate::games::Anchor::Bottom);
        let (fw, fh) = (864, 360);
        let mut frame = noise_frame(fw, fh, 11);
        // 240 px right of the centre, as on the 16:9 reference.
        paste(&mut frame, &tpl, fw / 2 + 240, 300, 1.0, 0.0);
        let anchored = PreparedDetector::from_images(d.clone(), vec![tpl.clone()], fw, fh).unwrap();
        assert!(anchored.score_frame(&frame) > 0.95);
        // Without the anchor the fractions land left of the element.
        d.anchor = None;
        let plain = PreparedDetector::from_images(d, vec![tpl], fw, fh).unwrap();
        assert!(plain.score_frame(&frame) < 0.6, "{}", plain.score_frame(&frame));
    }
}
