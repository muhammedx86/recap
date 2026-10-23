//! Template matching for vision detectors.
//!
//! Each detector looks only at its own small region of the frame. Inside that
//! region it slides each reference image and computes zero-mean normalized
//! cross-correlation (ZNCC), which is insensitive to brightness and contrast
//! changes. Work is bounded by downscaling so the template is at most
//! [`MAX_TEMPLATE_DIM`] pixels on its longest side.

use std::path::Path;

use anyhow::{Context, Result};
use image::{imageops::FilterType, GrayImage};

use crate::{
    events::{EventSource, GameEvent},
    games::{DetectorDef, LoadedDetector},
};

/// Longest side of a template after downscaling for matching.
pub const MAX_TEMPLATE_DIM: u32 = 96;

/// A reference image prepared for a specific analysis frame height.
#[derive(Debug, Clone)]
struct PreparedTemplate {
    img: GrayImage,
    /// Pixel values with the mean subtracted.
    zero_mean: Vec<f32>,
    /// sqrt(sum((t - mean)^2))
    norm: f32,
}

impl PreparedTemplate {
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
        Some(Self { img, zero_mean, norm })
    }
}

/// Best ZNCC score of `t` anywhere inside `hay`. Returns -1..1 (1 = identical up to brightness/contrast).
fn best_zncc(hay: &GrayImage, t: &PreparedTemplate) -> f32 {
    let (hw, hh) = hay.dimensions();
    let (tw, th) = t.img.dimensions();
    if tw > hw || th > hh {
        return -1.0;
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
    let mut best = -1f32;
    for y in 0..=(hh - th) as usize {
        for x in 0..=(hw - tw) as usize {
            let s = rect(&sum, x, y);
            let q = rect(&sq, x, y);
            let var = q - s * s / n;
            if var <= 1e-6 {
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
            if score > best {
                best = score;
            }
        }
    }
    best.min(1.0)
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
        for p in det.template_paths() {
            imgs.push(load_gray(&p)?);
        }
        Self::from_images(det.def.clone(), imgs, frame_w, frame_h)
    }

    pub fn from_images(def: DetectorDef, imgs: Vec<GrayImage>, frame_w: u32, frame_h: u32) -> Result<Self> {
        let base = frame_h as f32 / def.template_frame_height as f32;
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
        let mut templates = Vec::new();
        for img in &imgs {
            for s in &scales {
                let f = base * s * work_scale;
                let w = ((img.width() as f32 * f).round() as u32).max(2);
                let h = ((img.height() as f32 * f).round() as u32).max(2);
                let resized = image::imageops::resize(img, w, h, FilterType::Triangle);
                if let Some(t) = PreparedTemplate::new(resized) {
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

    /// Score a full grayscale frame (must be the size given at load time).
    pub fn score_frame(&self, frame: &GrayImage) -> f32 {
        let (x, y, w, h) = self.def.region.to_pixels(frame.width(), frame.height());
        let crop = image::imageops::crop_imm(frame, x, y, w, h).to_image();
        self.score_region(&crop)
    }

    /// Score an already-cropped region (at analysis resolution).
    pub fn score_region(&self, crop: &GrayImage) -> f32 {
        let hay = if self.work_scale < 0.999 {
            let w = ((crop.width() as f32 * self.work_scale).round() as u32).max(1);
            let h = ((crop.height() as f32 * self.work_scale).round() as u32).max(1);
            image::imageops::resize(crop, w, h, FilterType::Triangle)
        } else {
            crop.clone()
        };
        self.templates
            .iter()
            .map(|t| best_zncc(&pad_to(&hay, t.img.width(), t.img.height()), t))
            .fold(-1.0, f32::max)
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
    run: u32,
    run_start_ms: u64,
    run_peak: f32,
    fired_this_run: bool,
    last_fire_ms: Option<u64>,
}

impl Trigger {
    pub fn new(def: &DetectorDef) -> Self {
        Self {
            threshold: def.threshold,
            cooldown_ms: def.cooldown_ms,
            min_consecutive: def.min_consecutive.max(1),
            run: 0,
            run_start_ms: 0,
            run_peak: 0.0,
            fired_this_run: false,
            last_fire_ms: None,
        }
    }

    /// Feed one sample. Returns `Some((event_time_ms, peak_score))` when the detector fires.
    pub fn push(&mut self, t_ms: u64, score: f32) -> Option<(u64, f32)> {
        if score >= self.threshold {
            if self.run == 0 {
                self.run_start_ms = t_ms;
                self.run_peak = score;
                self.fired_this_run = false;
            }
            self.run += 1;
            self.run_peak = self.run_peak.max(score);
            if !self.fired_this_run && self.run >= self.min_consecutive {
                let cooled = self
                    .last_fire_ms
                    .is_none_or(|l| self.run_start_ms.saturating_sub(l) >= self.cooldown_ms);
                self.fired_this_run = true;
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
}
