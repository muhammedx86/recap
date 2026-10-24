//! Screen readers: fill match details (map, character, K/D/A...) from what is on screen.
//!
//! A reader looks at a box during part of the match (start, end or throughout), reads every
//! sampled frame, and the most common reading wins. Two kinds:
//!
//! * **text**: text recognition (a [`TextReader`], e.g. Windows OCR) of the box, kept only when it
//!   matches the reader's `pattern` and/or snaps to one of its `values`;
//! * **choice**: the labelled reference image that matches best (agent portraits, map names,
//!   VICTORY/DEFEAT), using the same matcher as the event detectors.
//!
//! A reader can also have repeated `slots` (one card per player) and a `highlight` colour that
//! picks the local player's slot.

use std::collections::BTreeMap;

use anyhow::Result;
use image::{imageops::FilterType, GrayImage, RgbImage};
use regex::Regex;
use serde::Serialize;

use crate::{
    details::{field, normalize_result, DetailSource, DetailValue, MatchDetails},
    ffmpeg::{ContentRect, Ffmpeg, VideoInfo},
    games::{parse_hex, DetectorDef, LoadedReader, ReadWhen, ReaderDef, ReaderKind, Region},
    vision::{load_gray, PreparedDetector},
};

/// Text recognition engine (Windows OCR in the app; anything in tests).
pub trait TextReader: Send + Sync {
    /// Read the text in a grayscale image (already cropped and cleaned up).
    fn read_text(&self, img: &GrayImage) -> Result<String>;
}

/// A reader with its references loaded for frames of one size.
pub struct PreparedReader {
    pub def: ReaderDef,
    pattern: Option<Regex>,
    /// Choice readers: (value, matcher) per option.
    options: Vec<(String, PreparedDetector)>,
    highlight: Option<([u8; 3], u8, Region)>,
    misses: std::sync::atomic::AtomicU32,
    fallbacks: std::sync::atomic::AtomicU32,
}

/// (text height px, margin as a multiple of the height) tried in order.
const OCR_ATTEMPTS: [(f32, f32); 4] = [(64.0, 0.25), (36.0, 0.25), (32.0, 1.0), (24.0, 2.0)];

impl PreparedReader {
    pub fn load(r: &LoadedReader, frame_w: u32, frame_h: u32) -> Result<Self> {
        let def = r.def.clone();
        let pattern = def.pattern.as_deref().map(Regex::new).transpose()?;
        let mut options = Vec::new();
        if def.kind == ReaderKind::Choice {
            for o in &def.options {
                let imgs: Vec<GrayImage> = o
                    .templates
                    .iter()
                    .filter_map(|t| {
                        load_gray(&r.base_dir.join(t))
                            .map_err(|e| log::warn!("reader {}: {e:#}", def.id))
                            .ok()
                    })
                    .collect();
                if imgs.is_empty() {
                    continue;
                }
                let det = choice_detector(&def, &o.value);
                match PreparedDetector::from_images(det, imgs, frame_w, frame_h) {
                    Ok(p) => options.push((o.value.clone(), p)),
                    Err(e) => log::warn!("reader {} option {}: {e:#}", def.id, o.value),
                }
            }
        }
        let highlight = match &def.highlight {
            Some(h) => Some((parse_hex(&h.color)?, h.tolerance, h.area)),
            None => None,
        };
        Ok(Self {
            def,
            pattern,
            options,
            highlight,
            misses: Default::default(),
            fallbacks: Default::default(),
        })
    }

    /// Which slot to read: the most highlighted one, or the first.
    pub fn pick_slot(&self, frame: &RgbImage) -> Option<u32> {
        let n = self.def.slot_count();
        let Some((color, tol, area)) = self.highlight else { return Some(0) };
        let mut best = (0.0f32, None);
        for i in 0..n {
            let slot = self.def.slot_region(i);
            let r = Region {
                x: slot.x + area.x * slot.w,
                y: slot.y + area.y * slot.h,
                w: (area.w * slot.w).max(1e-4),
                h: (area.h * slot.h).max(1e-4),
            };
            let (x, y, w, h) = r.to_pixels(frame.width(), frame.height());
            let mut hit = 0u32;
            for py in y..y + h {
                for px in x..x + w {
                    let p = frame.get_pixel(px, py).0;
                    if (0..3).all(|c| (p[c] as i16 - color[c] as i16).unsigned_abs() <= tol as u16) {
                        hit += 1;
                    }
                }
            }
            let frac = hit as f32 / (w * h).max(1) as f32;
            if frac > best.0 {
                best = (frac, Some(i));
            }
        }
        // Less than 1% of the area in that colour: nobody is highlighted on this frame.
        if best.0 >= 0.01 {
            best.1
        } else {
            None
        }
    }

    /// Read one frame (game picture, full resolution). Returns (field, value, confidence).
    pub fn read_frame(&self, frame: &RgbImage, ocr: Option<&dyn TextReader>) -> Vec<(String, String, f32)> {
        let Some(slot) = self.pick_slot(frame) else { return vec![] };
        let region = self.def.slot_region(slot);
        let (x, y, w, h) = region.to_pixels(frame.width(), frame.height());
        let gray = image::imageops::grayscale(&image::imageops::crop_imm(frame, x, y, w, h).to_image());
        match self.def.kind {
            ReaderKind::Choice => {
                let best = self
                    .options
                    .iter()
                    .map(|(v, d)| (v, d.score_region(&gray)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                match best {
                    Some((v, s)) if s >= self.def.threshold => finish(&self.def.field, v, s).into_iter().collect(),
                    _ => vec![],
                }
            }
            ReaderKind::Text => {
                let Some(ocr) = ocr else { return vec![] };
                // Nothing written there (a flat box): skip the OCR calls.
                if contrast(&gray) < 12.0 {
                    return vec![];
                }
                // OCR engines have a preferred text size and dislike text that fills the whole
                // image (short numbers especially): try a few sizes and margins.
                let mut seen = Vec::new();
                for (target, margin) in OCR_ATTEMPTS {
                    let text = match ocr.read_text(&prepare_for_ocr_with(&gray, target, margin)) {
                        Ok(t) => t,
                        Err(e) => {
                            log::debug!("reader {}: OCR failed: {e:#}", self.def.id);
                            continue;
                        }
                    };
                    let got = self.interpret(&text);
                    if !got.is_empty() {
                        if target != OCR_ATTEMPTS[0].0 {
                            self.note_fallback(target, margin);
                        }
                        return got;
                    }
                    seen.push(text);
                }
                // Last resort for short numbers, which Windows OCR ignores on their own: repeat
                // the box three times in a row and keep the reading that repeats.
                if self.pattern.as_ref().is_some_and(|p| p.as_str().contains("\\d")) {
                    let tiled = tile3(&prepare_for_ocr_with(&gray, 36.0, 0.0));
                    if let Ok(text) = ocr.read_text(&tiled) {
                        let fixed = fix_digits(&text);
                        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
                        for tok in fixed.split_whitespace() {
                            *counts.entry(tok).or_default() += 1;
                        }
                        if let Some((tok, n)) = counts.into_iter().max_by_key(|(_, n)| *n) {
                            let got = if n >= 2 { self.interpret(tok) } else { vec![] };
                            if !got.is_empty() {
                                self.note_fallback(36.0, -3.0);
                                return got;
                            }
                        }
                        seen.push(text);
                    }
                }
                self.note_miss(&seen);
                vec![]
            }
        }
    }

    fn note_fallback(&self, target: f32, margin: f32) {
        if self.fallbacks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 2 {
            log::info!("reader {}: read at {target} px with margin {margin}", self.def.id);
        }
    }

    /// Log a few unmatched readings per reader (helps tune boxes and patterns).
    fn note_miss(&self, seen: &[String]) {
        let n = self.misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 {
            log::info!("reader {}: no usable reading in {:?}", self.def.id, seen);
        }
    }

    /// Turn recognised text into field values (pattern, named groups, allowed values).
    pub fn interpret(&self, text: &str) -> Vec<(String, String, f32)> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        // Numbers: fix letters OCR confuses with digits inside number-like tokens ("L3" -> "13").
        let wants_digits = self.pattern.as_ref().is_some_and(|p| p.as_str().contains("\\d"));
        let text = if wants_digits { fix_digits(&text) } else { text };
        if text.is_empty() {
            return vec![];
        }
        let mut out = Vec::new();
        let base = match &self.pattern {
            Some(re) => {
                let Some(c) = re.captures(&text) else { return vec![] };
                let mut named = false;
                for name in re.capture_names().flatten() {
                    if let Some(m) = c.name(name) {
                        named = true;
                        out.extend(finish(name, m.as_str(), 1.0));
                    }
                }
                if named {
                    return out;
                }
                c.get(1).or_else(|| c.get(0)).map(|m| m.as_str().to_string()).unwrap_or_default()
            }
            None => text,
        };
        let value = if self.def.values.is_empty() {
            if base.chars().count() > 48 {
                return vec![];
            }
            Some((base, 1.0))
        } else {
            snap(&base, &self.def.values)
        };
        if let Some((v, conf)) = value {
            out.extend(finish(&self.def.field, &v, conf));
        }
        out
    }
}

/// A synthetic detector for one option of a choice reader (the matcher's settings).
fn choice_detector(def: &ReaderDef, value: &str) -> DetectorDef {
    DetectorDef {
        id: format!("{}-{}", def.id, crate::events::slugify(value)),
        label: value.to_string(),
        event: "_choice".into(),
        region: def.region,
        template_box: None,
        templates: vec![],
        template_frame_height: def.template_frame_height,
        method: def.method,
        threshold: def.threshold,
        cooldown_ms: 0,
        min_consecutive: 1,
        sample_fps: def.sample_fps,
        scales: vec![],
        rearm_drop: 0.0,
        enabled: true,
        notes: None,
    }
}

/// Field-specific clean-up: results normalised, numbers must be numbers.
fn finish(field_id: &str, value: &str, conf: f32) -> Option<(String, String, f32)> {
    let v = value.trim();
    let v = match field_id {
        field::RESULT => normalize_result(v)?.to_string(),
        field::KILLS | field::DEATHS | field::ASSISTS => {
            // Common OCR slips in digits.
            let fixed: String = v
                .chars()
                .map(|c| match c {
                    'O' | 'o' | 'D' => '0',
                    'l' | 'I' | '|' => '1',
                    'S' | 's' => '5',
                    'B' => '8',
                    c => c,
                })
                .collect();
            fixed.parse::<u32>().ok()?.to_string()
        }
        _ => v.to_string(),
    };
    (!v.is_empty()).then(|| (field_id.to_string(), v, conf))
}

/// In tokens that are at least half digits, replace look-alike letters with digits.
pub fn fix_digits(text: &str) -> String {
    text.split(' ')
        .map(|tok| {
            let digits = tok.chars().filter(|c| c.is_ascii_digit()).count();
            let letters = tok.chars().filter(|c| c.is_alphabetic() || *c == '|').count();
            if digits == 0 || digits < letters {
                return tok.to_string();
            }
            tok.chars()
                .map(|c| match c {
                    'O' | 'o' | 'D' | 'Q' => '0',
                    'l' | 'I' | 'i' | '|' | 'L' | 'j' | 'J' => '1',
                    'Z' | 'z' => '2',
                    'S' | 's' => '5',
                    'G' | 'b' => '6',
                    'B' => '8',
                    'g' | 'q' => '9',
                    c => c,
                })
                .collect()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn norm_key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Snap a reading to the closest allowed value (case/punctuation-insensitive), or None if it is
/// not close to any. Confidence drops with the edit distance.
pub fn snap(reading: &str, values: &[String]) -> Option<(String, f32)> {
    let r: Vec<char> = norm_key(reading).chars().collect();
    if r.is_empty() {
        return None;
    }
    let mut best: Option<(usize, &String, usize)> = None;
    for v in values {
        let k: Vec<char> = norm_key(v).chars().collect();
        if k.is_empty() {
            continue;
        }
        // Allow the value to appear inside a longer reading ("MAP ASCENT").
        let d = if r.windows(k.len()).any(|w| w == k.as_slice()) {
            0
        } else {
            levenshtein(&r, &k)
        };
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, v, k.len()));
        }
    }
    let (d, v, len) = best?;
    let allowed = (len / 4).max(1);
    (d <= allowed).then(|| (v.clone(), 1.0 - d as f32 / (len as f32 + 1.0)))
}

/// Make a crop easy to read: at least 64 px tall, dark text on a light background, full contrast.
pub fn prepare_for_ocr(gray: &GrayImage) -> GrayImage {
    prepare_for_ocr_at(gray, 64.0)
}

/// [`prepare_for_ocr`] with the box scaled to about `target` px tall.
pub fn prepare_for_ocr_at(gray: &GrayImage, target: f32) -> GrayImage {
    prepare_for_ocr_with(gray, target, 0.25)
}

/// [`prepare_for_ocr_at`] with a quiet margin of `margin` x the height on every side.
pub fn prepare_for_ocr_with(gray: &GrayImage, target: f32, margin: f32) -> GrayImage {
    let (w, h) = gray.dimensions();
    let scale = (target / h.max(1) as f32).clamp(0.3, 4.0);
    let mut img = if (scale - 1.0).abs() > 0.01 {
        image::imageops::resize(
            gray,
            (w as f32 * scale).round() as u32,
            (h as f32 * scale).round() as u32,
            FilterType::CatmullRom,
        )
    } else {
        gray.clone()
    };
    // Contrast stretch between the 2nd and 98th percentile.
    let mut hist = [0u32; 256];
    for p in img.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let total: u32 = hist.iter().sum();
    let pct = |q: f32| {
        let target = (total as f32 * q) as u32;
        let mut acc = 0;
        for (i, &c) in hist.iter().enumerate() {
            acc += c;
            if acc >= target {
                return i as f32;
            }
        }
        255.0
    };
    let (lo, hi) = (pct(0.02), pct(0.98).max(pct(0.02) + 1.0));
    let mean: f32 = img.pixels().map(|p| p.0[0] as f32).sum::<f32>() / total.max(1) as f32;
    // Text is the minority: if the picture is mostly dark, the text is light, so invert.
    let invert = mean < (lo + hi) / 2.0;
    for p in img.pixels_mut() {
        let v = ((p.0[0] as f32 - lo) / (hi - lo) * 255.0).clamp(0.0, 255.0);
        p.0[0] = if invert { 255 - v as u8 } else { v as u8 };
    }
    // Black text on white: drops faint scenery behind the text.
    let t = otsu(&img);
    for p in img.pixels_mut() {
        p.0[0] = if p.0[0] > t { 255 } else { 0 };
    }
    // OCR engines want a quiet margin around the text (a tight box touches the glyphs).
    let pad = ((img.height() as f32 * margin) as u32).max(8);
    let mut out = GrayImage::from_pixel(img.width() + 2 * pad, img.height() + 2 * pad, image::Luma([255]));
    image::imageops::replace(&mut out, &img, pad as i64, pad as i64);
    out
}

/// Three copies of the text in a prepared (black on white) image, trimmed to the ink and joined
/// with normal word spacing so they read as one line ("13 13 13").
fn tile3(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[0] < 128 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x1 < x0 || y1 < y0 {
        return img.clone();
    }
    let ink = image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image();
    let (iw, ih) = ink.dimensions();
    let gap = (ih as f32 * 0.45) as u32 + 2;
    let margin = (ih / 2).max(8);
    let mut out = GrayImage::from_pixel(iw * 3 + gap * 2 + margin * 2, ih + margin * 2, image::Luma([255]));
    for i in 0..3 {
        image::imageops::replace(&mut out, &ink, (margin + i * (iw + gap)) as i64, margin as i64);
    }
    out
}

/// Standard deviation of the pixel values.
fn contrast(img: &GrayImage) -> f32 {
    let n = (img.width() * img.height()).max(1) as f32;
    let mean = img.pixels().map(|p| p.0[0] as f32).sum::<f32>() / n;
    (img.pixels().map(|p| (p.0[0] as f32 - mean).powi(2)).sum::<f32>() / n).sqrt()
}

/// Otsu's threshold of a grayscale image.
fn otsu(img: &GrayImage) -> u8 {
    let mut hist = [0f64; 256];
    for p in img.pixels() {
        hist[p.0[0] as usize] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let sum_all: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * c).sum();
    let (mut w_b, mut sum_b, mut best, mut t) = (0.0, 0.0, -1.0, 127u8);
    for (i, c) in hist.iter().enumerate() {
        w_b += c;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += i as f64 * c;
        let (m_b, m_f) = (sum_b / w_b, (sum_all - sum_b) / w_f);
        let between = w_b * w_f * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            t = i as u8;
        }
    }
    t
}

/// Time ranges (ms into the video) for each `when`.
#[derive(Debug, Clone, Copy)]
pub struct ReadWindows {
    pub start: (u64, u64),
    pub end: (u64, u64),
    pub any: (u64, u64),
}

impl ReadWindows {
    /// Windows for a match in `[match_start, match_end]` of a video `duration_ms` long, given
    /// optional match_start / match_end event times and where the next match starts (if any).
    pub fn for_match(duration_ms: u64, start_ev: Option<u64>, end_ev: Option<u64>, next_start: Option<u64>) -> Self {
        let s = start_ev.unwrap_or(0);
        let limit = next_start.unwrap_or(duration_ms).min(duration_ms);
        let end = match end_ev {
            // From just before the end banner to the next match (end screens can last a while).
            Some(e) => (e.saturating_sub(5_000), limit.min(e + 120_000)),
            None => (limit.saturating_sub(90_000), limit),
        };
        Self {
            start: (s, (s + 60_000).min(limit)),
            end,
            any: (s, end.1),
        }
    }

    fn get(&self, w: ReadWhen) -> (u64, u64) {
        match w {
            ReadWhen::Start => self.start,
            ReadWhen::End => self.end,
            ReadWhen::Any => self.any,
        }
    }
}

/// What each reader saw, for Detector Studio's test view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderTrace {
    pub reader: String,
    /// (t_ms, field, value) for every frame that produced something.
    pub readings: Vec<(u64, String, String)>,
    pub frames: u32,
}

/// Run `readers` over `video` and vote. Frames are decoded once per distinct time window.
pub fn read_details(
    ff: &Ffmpeg,
    video: &std::path::Path,
    readers: &[LoadedReader],
    windows: &ReadWindows,
    ocr: Option<&dyn TextReader>,
    traces: Option<&mut Vec<ReaderTrace>>,
) -> Result<MatchDetails> {
    let info: VideoInfo = ff.probe(video)?;
    let crop: Option<ContentRect> = ff.content_rect(video, &info);
    let (fw, fh) = crop.map(|c| (c.w, c.h)).unwrap_or((info.width, info.height));
    let ready: Vec<&LoadedReader> = readers.iter().filter(|r| r.def.is_ready()).collect();
    let mut prepared = Vec::new();
    for r in ready {
        match PreparedReader::load(r, fw, fh) {
            Ok(p) => prepared.push(p),
            Err(e) => log::warn!("reader {}: {e:#}", r.def.id),
        }
    }
    // Group readers by (window, fps) so each range is decoded once.
    let mut groups: BTreeMap<(u64, u64, u32), Vec<&PreparedReader>> = BTreeMap::new();
    for p in &prepared {
        let (a, b) = windows.get(p.def.when);
        if b <= a {
            continue;
        }
        let fps_milli = (p.def.sample_fps * 1000.0).round() as u32;
        groups.entry((a, b, fps_milli)).or_default().push(p);
    }
    // field -> value -> (votes, confidence sum)
    let mut votes: BTreeMap<String, BTreeMap<String, (u32, f32)>> = BTreeMap::new();
    let mut local_traces: Vec<ReaderTrace> = prepared
        .iter()
        .map(|p| ReaderTrace {
            reader: p.def.id.clone(),
            readings: vec![],
            frames: 0,
        })
        .collect();
    for ((a, b, fps_milli), rs) in groups {
        let fps = fps_milli as f32 / 1000.0;
        for (t, frame) in ff.rgb_frames(video, a, b - a, fps, &info, crop)? {
            for r in &rs {
                let found = r.read_frame(&frame, ocr);
                if let Some(tr) = local_traces.iter_mut().find(|x| x.reader == r.def.id) {
                    tr.frames += 1;
                    tr.readings.extend(found.iter().map(|(f, v, _)| (t, f.clone(), v.clone())));
                }
                for (f, v, c) in found {
                    let e = votes.entry(f).or_default().entry(v).or_insert((0, 0.0));
                    e.0 += 1;
                    e.1 += c;
                }
            }
        }
    }
    if let Some(t) = traces {
        *t = local_traces;
    }
    let mut out = MatchDetails::new();
    for (f, vals) in votes {
        let total: u32 = vals.values().map(|v| v.0).sum();
        if let Some((v, (n, c))) = vals.into_iter().max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(a.1 .1.total_cmp(&b.1 .1))) {
            let conf = (n as f32 / total as f32) * (c / n as f32);
            out.insert(
                f,
                DetailValue::new(v, DetailSource::Screen).with_confidence((conf * 100.0).round() / 100.0),
            );
        }
    }
    crate::details::combine_team_scores(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::{ChoiceOption, Highlight, Slots};

    fn reader(kind: ReaderKind, field: &str) -> ReaderDef {
        ReaderDef {
            id: "r".into(),
            label: String::new(),
            field: field.into(),
            kind,
            region: Region {
                x: 0.1,
                y: 0.1,
                w: 0.2,
                h: 0.1,
            },
            slots: None,
            highlight: None,
            when: ReadWhen::End,
            sample_fps: 1.0,
            pattern: None,
            values: vec![],
            options: vec![],
            threshold: 0.75,
            method: Default::default(),
            template_frame_height: 1080,
            enabled: true,
            notes: None,
        }
    }

    fn prepared(def: ReaderDef) -> PreparedReader {
        PreparedReader::load(
            &LoadedReader {
                def,
                base_dir: std::env::temp_dir(),
                user: false,
            },
            640,
            360,
        )
        .unwrap()
    }

    #[test]
    fn interprets_kda_maps_and_results() {
        let mut d = reader(ReaderKind::Text, "kda");
        d.pattern = Some(r"(?P<kills>\d+)\s*/\s*(?P<deaths>\d+)\s*/\s*(?P<assists>\d+)".into());
        let p = prepared(d);
        let got = p.interpret(" 18 / 6 /  8 ");
        assert_eq!(
            got.iter().map(|(f, v, _)| (f.as_str(), v.as_str())).collect::<Vec<_>>(),
            vec![("kills", "18"), ("deaths", "6"), ("assists", "8")]
        );
        assert!(p.interpret("COMBAT SCORE").is_empty());

        let mut m = reader(ReaderKind::Text, "map");
        m.values = ["Ascent", "Bind", "Split", "Icebox", "The Range"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let p = prepared(m);
        assert_eq!(p.interpret("SPLIT")[0].1, "Split");
        assert_eq!(p.interpret("1CEBOX")[0].1, "Icebox", "one OCR slip is fine");
        assert_eq!(p.interpret("MAP: ASCENT")[0].1, "Ascent");
        assert!(p.interpret("COMPETITIVE").is_empty());

        let mut sc = reader(ReaderKind::Text, "team_score");
        sc.pattern = Some(r"^\s*(\d{1,2})\s*$".into());
        let p = prepared(sc);
        assert_eq!(p.interpret("L3")[0].1, "13");
        assert_eq!(p.interpret("1O")[0].1, "10");
        assert!(p.interpret("LOL").is_empty());
        assert_eq!(fix_digits("3/l/2 KDA"), "3/1/2 KDA");

        let p = prepared(reader(ReaderKind::Text, "result"));
        assert_eq!(p.interpret("VICTORY")[0].1, "win");
        assert!(p.interpret("CONTINUE").is_empty());
    }

    /// Mimics Windows OCR: nothing for a short number alone, fine once it is part of a long line.
    struct ShyOcr;
    impl TextReader for ShyOcr {
        fn read_text(&self, img: &GrayImage) -> Result<String> {
            Ok(if img.width() > 2 * img.height() {
                "13 13 L3".into()
            } else {
                String::new()
            })
        }
    }

    #[test]
    fn short_numbers_are_read_from_a_tiled_line() {
        let mut d = reader(ReaderKind::Text, "team_score");
        d.pattern = Some(r"^\s*(\d{1,2})\s*$".into());
        d.region = Region {
            x: 0.0,
            y: 0.0,
            w: 0.2,
            h: 0.2,
        };
        let p = prepared(d);
        let mut frame = RgbImage::from_pixel(640, 360, image::Rgb([20, 20, 30]));
        for y in 10..50 {
            for x in 10..50 {
                frame.put_pixel(x, y, image::Rgb([200, 230, 200]));
            }
        }
        let got = p.read_frame(&frame, Some(&ShyOcr));
        assert_eq!(got.first().map(|g| g.1.as_str()), Some("13"));
    }

    #[test]
    fn highlighted_slot_is_picked() {
        let mut d = reader(ReaderKind::Text, "kda");
        d.slots = Some(Slots {
            count: 3,
            step_x: 0.25,
            step_y: 0.0,
        });
        d.highlight = Some(Highlight {
            area: Region {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 0.5,
            },
            color: "#ebe86a".into(),
            tolerance: 30,
        });
        let p = prepared(d);
        let mut frame = RgbImage::from_pixel(640, 360, image::Rgb([20, 20, 30]));
        assert_eq!(p.pick_slot(&frame), None);
        // Paint part of slot 2 (x from 0.6 to 0.8) yellow.
        for y in 40..50 {
            for x in 400..480 {
                frame.put_pixel(x, y, image::Rgb([235, 230, 110]));
            }
        }
        assert_eq!(p.pick_slot(&frame), Some(2));
    }

    #[test]
    fn choice_reader_picks_best_option() {
        let dir = std::env::temp_dir().join(format!("recap-choice-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        // Two "icons": a ring and a cross, 40x40, drawn at 360p.
        let ring = GrayImage::from_fn(40, 40, |x, y| {
            let d = ((x as f32 - 20.0).powi(2) + (y as f32 - 20.0).powi(2)).sqrt();
            image::Luma([if (d - 13.0).abs() < 3.0 { 240 } else { 20 }])
        });
        let cross = GrayImage::from_fn(40, 40, |x, y| {
            image::Luma([if (x as i32 - 20).abs() < 4 || (y as i32 - 20).abs() < 4 {
                240
            } else {
                20
            }])
        });
        ring.save(dir.join("templates/ring.png")).unwrap();
        cross.save(dir.join("templates/cross.png")).unwrap();
        let mut d = reader(ReaderKind::Choice, "character");
        d.region = Region {
            x: 100.0 / 640.0,
            y: 100.0 / 360.0,
            w: 60.0 / 640.0,
            h: 60.0 / 360.0,
        };
        d.template_frame_height = 360;
        d.options = vec![
            ChoiceOption {
                value: "Ring".into(),
                templates: vec!["templates/ring.png".into()],
            },
            ChoiceOption {
                value: "Cross".into(),
                templates: vec!["templates/cross.png".into()],
            },
        ];
        let p = PreparedReader::load(
            &LoadedReader {
                def: d,
                base_dir: dir.clone(),
                user: false,
            },
            640,
            360,
        )
        .unwrap();
        let mut frame = RgbImage::from_pixel(640, 360, image::Rgb([20, 20, 20]));
        for (x, y, v) in cross.enumerate_pixels() {
            frame.put_pixel(108 + x, 110 + y, image::Rgb([v.0[0]; 3]));
        }
        let got = p.read_frame(&frame, None);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].1.as_str()), ("character", "Cross"));
        assert!(p
            .read_frame(&RgbImage::from_pixel(640, 360, image::Rgb([20, 20, 20])), None)
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ocr_prep_makes_dark_text_on_light() {
        // Light text (200) on dark (30), 20 px tall.
        let img = GrayImage::from_fn(60, 20, |x, _| image::Luma([if (20..30).contains(&x) { 200 } else { 30 }]));
        let out = prepare_for_ocr(&img);
        assert!(out.height() >= 64);
        assert!(out.get_pixel(2, 2).0[0] > 200, "background becomes light");
        assert!(out.get_pixel(out.width() / 2 - 4, out.height() / 2).0[0] < 60, "text becomes dark");
        assert_eq!(snap("Haven", &["Bind".into()]), None);
    }

    #[test]
    fn windows() {
        let w = ReadWindows::for_match(600_000, Some(10_000), Some(500_000), None);
        assert_eq!(w.start, (10_000, 70_000));
        assert_eq!(w.end, (495_000, 600_000));
        let w = ReadWindows::for_match(600_000, None, None, Some(400_000));
        assert_eq!(w.end, (310_000, 400_000));
    }
}
