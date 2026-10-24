//! A scripted fake game for Recap's end-to-end test.
//!
//! Everything here is deterministic: [`render_frame`] draws the frame shown at a
//! given time, so the windowed game, the exported fixture templates and the
//! exported test frames all come from the same code. Banners are opaque,
//! text-free abstract patterns at fixed boxes, shown at the times in [`timeline`]
//! (see docs/E2E.md).

use std::{fs, path::Path};

pub mod endscreen;

use anyhow::{Context, Result};
use recap_core::games::{AutoClipDef, CaptureMode, DetectMethod, DetectorDef, GameDef, Region};
use serde::Serialize;

pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 720;
/// The game exits on its own after this many seconds.
pub const EXIT_AT_S: f64 = 64.0;
pub const GAME_ID: &str = "recap-sample-game";
pub const PROCESS_NAME: &str = "recap-sample-game.exe";
pub const WINDOW_TITLE: &str = "Recap Sample Game";

/// The six banner kinds; the string is the Recap event kind they mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Banner {
    MatchStart,
    RoundStart,
    Kill,
    RoundWin,
    RoundLoss,
    MatchEnd,
}

pub const ALL_BANNERS: [Banner; 6] = [
    Banner::MatchStart,
    Banner::RoundStart,
    Banner::Kill,
    Banner::RoundWin,
    Banner::RoundLoss,
    Banner::MatchEnd,
];

impl Banner {
    /// Recap event kind.
    pub fn kind(self) -> &'static str {
        match self {
            Banner::MatchStart => "match_start",
            Banner::RoundStart => "round_start",
            Banner::Kill => "kill",
            Banner::RoundWin => "round_win",
            Banner::RoundLoss => "round_loss",
            Banner::MatchEnd => "match_end",
        }
    }

    /// Detector id in the fixture.
    pub fn detector_id(self) -> &'static str {
        match self {
            Banner::MatchStart => "match-start",
            Banner::RoundStart => "round-start",
            Banner::Kill => "kill",
            Banner::RoundWin => "round-won",
            Banner::RoundLoss => "round-lost",
            Banner::MatchEnd => "match-end",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Banner::MatchStart => "Match start banner",
            Banner::RoundStart => "Round start banner",
            Banner::Kill => "Kill banner",
            Banner::RoundWin => "Round won banner",
            Banner::RoundLoss => "Round lost banner",
            Banner::MatchEnd => "Match end screen",
        }
    }

    /// Fixed on-screen box (x, y, w, h) in 1280x720 pixels. Round won/lost share a box.
    pub fn rect(self) -> (usize, usize, usize, usize) {
        match self {
            Banner::RoundStart => (560, 28, 160, 48),
            Banner::MatchStart => (480, 120, 320, 90),
            Banner::MatchEnd => (480, 290, 320, 100),
            Banner::RoundWin | Banner::RoundLoss => (520, 460, 240, 60),
            Banner::Kill => (580, 590, 120, 60),
        }
    }

    /// The box as fractions of the frame.
    pub fn region(self) -> Region {
        let (x, y, w, h) = self.rect();
        Region {
            x: x as f32 / WIDTH as f32,
            y: y as f32 / HEIGHT as f32,
            w: w as f32 / WIDTH as f32,
            h: h as f32 / HEIGHT as f32,
        }
    }

    /// Luma (0..255) of the banner at local coordinates.
    fn luma(self, x: i32, y: i32, w: i32, h: i32) -> u8 {
        match self {
            // A ring above a bar.
            Banner::Kill => {
                let (dx, dy) = ((x - 60) as f32, (y - 26) as f32);
                let d = (dx * dx + dy * dy).sqrt();
                let ring = (d - 18.0).abs() <= 3.0;
                let bar = (45..=52).contains(&y) && (18..=101).contains(&x);
                if ring || bar {
                    235
                } else {
                    28
                }
            }
            // Three posts and a pennant.
            Banner::RoundStart => {
                let post = (8..=40).contains(&y) && [16, 44, 72].iter().any(|&p| (p..p + 14).contains(&x));
                let flag = (100..=150).contains(&x) && {
                    let t = (x - 100) as f32 / 50.0;
                    let half = 18.0 * (1.0 - t);
                    ((y - 24) as f32).abs() <= half
                };
                if post || flag {
                    220
                } else {
                    36
                }
            }
            // Concentric diamonds.
            Banner::RoundWin => {
                let dx = (x - w / 2).abs() as f32 / (w as f32 / 2.0);
                let dy = (y - h / 2).abs() as f32 / (h as f32 / 2.0);
                let r = dx + dy;
                if ((r * 3.0) as i32) % 2 == 0 {
                    228
                } else {
                    30
                }
            }
            // Wide diagonal stripes.
            Banner::RoundLoss => {
                if ((x + 2 * y) / 24) % 2 == 0 {
                    205
                } else {
                    44
                }
            }
            // Two bars with three forward chevrons.
            Banner::MatchStart => {
                let bars = (6..=18).contains(&y) || (h - 19..=h - 7).contains(&y);
                let cy = h / 2;
                let chevron = [70, 140, 210].iter().any(|&cx| {
                    let ax = x - cx;
                    (0..=40).contains(&ax) && ((y - cy).abs() - (40 - ax)).abs() <= 7
                });
                if bars || chevron {
                    240
                } else {
                    22
                }
            }
            // Bright screen with a dark disc and a dark slab.
            Banner::MatchEnd => {
                let (dx, dy) = ((x - 70) as f32, (y - h / 2) as f32);
                let disc = dx * dx + dy * dy <= 34.0 * 34.0;
                let slab = (140..=290).contains(&x) && (30..=70).contains(&y);
                if disc || slab {
                    24
                } else {
                    226
                }
            }
        }
    }

    /// A slight colour cast per banner (luma stays the pattern).
    fn tint(self) -> (i32, i32, i32) {
        match self {
            Banner::Kill => (-10, 8, 2),
            Banner::RoundStart => (0, 0, 10),
            Banner::RoundWin => (-8, 10, -8),
            Banner::RoundLoss => (12, -6, -6),
            Banner::MatchStart => (10, 6, -12),
            Banner::MatchEnd => (6, 6, 6),
        }
    }
}

/// One scheduled banner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    #[serde(rename = "tS")]
    pub t_s: f64,
    pub duration_s: f64,
    #[serde(serialize_with = "ser_kind")]
    pub kind: Banner,
}

fn ser_kind<S: serde::Serializer>(b: &Banner, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(b.kind())
}

impl Cue {
    pub fn end_s(&self) -> f64 {
        self.t_s + self.duration_s
    }
}

/// The scripted timeline (docs/E2E.md).
pub fn timeline() -> Vec<Cue> {
    use Banner::*;
    let c = |t_s: f64, duration_s: f64, kind: Banner| Cue { t_s, duration_s, kind };
    vec![
        c(10.0, 2.0, MatchStart),
        c(12.5, 1.2, RoundStart),
        c(15.0, 1.2, Kill),
        c(18.0, 1.2, Kill),
        c(20.0, 1.5, RoundWin),
        c(22.0, 1.2, RoundStart),
        c(24.0, 1.2, Kill),
        c(26.5, 1.5, RoundLoss),
        c(29.0, 2.0, MatchEnd),
        c(45.0, 2.0, MatchStart),
        c(47.5, 1.2, RoundStart),
        c(50.0, 1.2, Kill),
        c(53.0, 1.5, RoundWin),
        c(56.0, 2.0, MatchEnd),
    ]
}

/// True between a match's start banner and the end of its end banner.
fn in_match(t: f64, cues: &[Cue]) -> bool {
    let mut inside = false;
    for c in cues {
        match c.kind {
            Banner::MatchStart if t >= c.t_s => inside = true,
            Banner::MatchEnd if t >= c.end_s() => inside = false,
            _ => {}
        }
    }
    inside
}

#[inline]
pub(crate) fn rgb(r: i32, g: i32, b: i32) -> u32 {
    let c = |v: i32| v.clamp(0, 255) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// Cheap deterministic per-pixel noise in -range..=range.
#[inline]
fn noise(x: usize, y: usize, frame: u32, range: i32) -> i32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ frame.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h % (2 * range as u32 + 1)) as i32 - range
}

/// Draw the frame shown at `t` seconds into `buf` (WIDTH*HEIGHT, 0RGB).
/// Area-average a full-size frame into a smaller `w`×`h` 0RGB buffer.
pub fn downscale(src: &[u32], dst: &mut [u32], w: usize, h: usize) {
    for dy in 0..h {
        let (y0, y1) = (dy * HEIGHT / h, ((dy + 1) * HEIGHT / h).max(dy * HEIGHT / h + 1));
        for dx in 0..w {
            let (x0, x1) = (dx * WIDTH / w, ((dx + 1) * WIDTH / w).max(dx * WIDTH / w + 1));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for &p in &src[y * WIDTH + x0..y * WIDTH + x1] {
                    r += (p >> 16) & 0xff;
                    g += (p >> 8) & 0xff;
                    b += p & 0xff;
                    n += 1;
                }
            }
            dst[dy * w + dx] = ((r / n) << 16) | ((g / n) << 8) | (b / n);
        }
    }
}

pub fn render_frame(t: f64, buf: &mut [u32]) {
    assert_eq!(buf.len(), WIDTH * HEIGHT);
    let cues = timeline();
    let playing = in_match(t, &cues);
    let frame = (t * 60.0) as u32;

    // Background: a drifting gradient; the match scene is warmer than the menu.
    let col: Vec<i32> = (0..WIDTH).map(|x| (18.0 * ((x as f64 / 140.0) + t * 0.9).sin()) as i32).collect();
    let (base_r, base_g, base_b) = if playing { (52, 44, 40) } else { (30, 36, 54) };
    for y in 0..HEIGHT {
        let row = (14.0 * ((y as f64 / 90.0) - t * 0.6).cos()) as i32;
        let line = &mut buf[y * WIDTH..(y + 1) * WIDTH];
        for (x, px) in line.iter_mut().enumerate() {
            let v = col[x] + row + noise(x, y, frame, 5);
            *px = rgb(base_r + v, base_g + v, base_b + v);
        }
    }

    // Moving shapes so consecutive frames always differ.
    for i in 0..3usize {
        let speed = 90.0 + 55.0 * i as f64;
        let w = 150 + 60 * i;
        let h = 70 + 30 * i;
        let span = (WIDTH + w) as f64;
        let x0 = ((t * speed + 400.0 * i as f64) % span) as i64 - w as i64;
        let y0 = 160 + 150 * i as i64 + (25.0 * (t * 0.7 + i as f64).sin()) as i64;
        let shade = if playing { 78 + 8 * i as i32 } else { 62 + 6 * i as i32 };
        fill_rect(buf, x0, y0, w as i64, h as i64, rgb(shade, shade - 4, shade - 8));
    }

    // Menu: a static panel of list items on the left.
    if !playing {
        fill_rect(buf, 60, 140, 260, 420, rgb(24, 28, 40));
        for i in 0..5 {
            fill_rect(buf, 84, 170 + 76 * i, 212, 40, rgb(46, 52, 70));
        }
    }

    // Banners (opaque).
    for c in cues.iter().filter(|c| t >= c.t_s && t < c.end_s()) {
        draw_banner(buf, c.kind, frame);
    }

    // End-of-match screen (covers everything).
    for s in endscreen::end_screens().iter().filter(|s| t >= s.t_s && t < s.t_s + s.duration_s) {
        endscreen::draw_end_screen(buf, s);
    }
}

pub(crate) fn fill_rect(buf: &mut [u32], x0: i64, y0: i64, w: i64, h: i64, color: u32) {
    let xs = x0.max(0) as usize;
    let xe = (x0 + w).clamp(0, WIDTH as i64) as usize;
    let ys = y0.max(0) as usize;
    let ye = (y0 + h).clamp(0, HEIGHT as i64) as usize;
    for y in ys..ye {
        buf[y * WIDTH + xs..y * WIDTH + xe].fill(color);
    }
}

fn draw_banner(buf: &mut [u32], b: Banner, frame: u32) {
    let (bx, by, w, h) = b.rect();
    let (tr, tg, tb) = b.tint();
    for y in 0..h {
        for x in 0..w {
            let l = b.luma(x as i32, y as i32, w as i32, h as i32) as i32 + noise(bx + x, by + y, frame, 3);
            buf[(by + y) * WIDTH + bx + x] = rgb(l + tr, l + tg, l + tb);
        }
    }
}

pub fn new_frame() -> Vec<u32> {
    vec![0u32; WIDTH * HEIGHT]
}

/// Convert a 0RGB frame to an RGB image.
pub fn to_image(buf: &[u32]) -> image::RgbImage {
    image::RgbImage::from_fn(WIDTH as u32, HEIGHT as u32, |x, y| {
        let p = buf[y as usize * WIDTH + x as usize];
        image::Rgb([(p >> 16) as u8, (p >> 8) as u8, p as u8])
    })
}

/// The fixture's game definition.
pub fn game_def() -> GameDef {
    GameDef {
        id: GAME_ID.into(),
        name: "Sample Game".into(),
        processes: vec![PROCESS_NAME.into()],
        capture: CaptureMode::Window,
        api: None,
        prefer_api: true,
        auto_clip: AutoClipDef {
            events: vec!["kill".into()],
        },
        post_match: vec![],
        color: Some("#7cc3a3".into()),
        event_types: vec![],
        rounds: None,
    }
}

/// The fixture's detector for one banner.
pub fn detector_def(b: Banner) -> DetectorDef {
    DetectorDef {
        id: b.detector_id().into(),
        label: b.label().into(),
        event: b.kind().into(),
        region: b.region().expanded(0.25),
        template_box: Some(b.region()),
        templates: vec![format!("templates/{}.png", b.detector_id())],
        template_frame_height: HEIGHT as u32,
        method: DetectMethod::Ncc,
        threshold: 0.8,
        cooldown_ms: if b == Banner::Kill { 1000 } else { 3000 },
        min_consecutive: 2,
        sample_fps: 6.0,
        scales: vec![],
        rearm_drop: 0.0,
        hold_margin: 0.0,
        enabled: true,
        notes: Some("Sample game fixture for Recap's end-to-end test.".into()),
    }
}

/// Write `<dir>/recap-sample-game/` (game.json, detectors, templates). Returns the game folder.
pub fn export_fixture(dir: &Path) -> Result<std::path::PathBuf> {
    let game_dir = dir.join(GAME_ID);
    fs::create_dir_all(game_dir.join("detectors"))?;
    fs::create_dir_all(game_dir.join("templates"))?;
    fs::write(game_dir.join("game.json"), serde_json::to_string_pretty(&game_def())?)?;
    let cues = timeline();
    let mut buf = new_frame();
    for b in ALL_BANNERS {
        let det = detector_def(b);
        det.validate().with_context(|| format!("detector {}", det.id))?;
        let cue = cues.iter().find(|c| c.kind == b).context("banner never shown")?;
        render_frame(cue.t_s + cue.duration_s / 2.0, &mut buf);
        let (x, y, w, h) = b.rect();
        let crop = image::imageops::crop_imm(&to_image(&buf), x as u32, y as u32, w as u32, h as u32).to_image();
        crop.save(game_dir.join("templates").join(format!("{}.png", b.detector_id())))?;
        fs::write(
            game_dir.join("detectors").join(format!("{}.json", b.detector_id())),
            serde_json::to_string_pretty(&det)?,
        )?;
    }
    // Screen readers for the end screen, and the character icon references.
    fs::create_dir_all(game_dir.join("readers"))?;
    for r in endscreen::readers() {
        r.validate().with_context(|| format!("reader {}", r.id))?;
        fs::write(
            game_dir.join("readers").join(format!("{}.json", r.id)),
            serde_json::to_string_pretty(&r)?,
        )?;
    }
    for icon in [endscreen::Icon::Ring, endscreen::Icon::Cross] {
        endscreen::icon_image(icon).save(game_dir.join("templates").join(format!("icon-{}.png", icon.name().to_lowercase())))?;
    }
    Ok(game_dir)
}

/// Render the whole timeline as PNG frames `frame_00000.png`… at `fps`.
pub fn export_frames(dir: &Path, fps: f64) -> Result<usize> {
    anyhow::ensure!(fps > 0.0 && fps <= 120.0, "fps must be in (0, 120]");
    fs::create_dir_all(dir)?;
    let n = (EXIT_AT_S * fps).ceil() as usize;
    let mut buf = new_frame();
    for i in 0..n {
        render_frame(i as f64 / fps, &mut buf);
        to_image(&buf).save(dir.join(format!("frame_{i:05}.png")))?;
    }
    Ok(n)
}

pub fn timeline_json() -> String {
    serde_json::to_string_pretty(&timeline()).expect("timeline serialises")
}

#[cfg(test)]
mod tests {
    #[test]
    fn downscale_averages() {
        let src = vec![0x00ff_8040u32; super::WIDTH * super::HEIGHT];
        let mut dst = vec![0u32; 960 * 540];
        super::downscale(&src, &mut dst, 960, 540);
        assert!(dst.iter().all(|&p| p == 0x00ff_8040));
    }

    use super::*;

    #[test]
    fn timeline_is_ordered_and_banners_never_overlap() {
        let t = timeline();
        assert_eq!(t.len(), 14);
        for w in t.windows(2) {
            assert!(w[0].end_s() <= w[1].t_s, "{:?} overlaps {:?}", w[0], w[1]);
        }
        assert!(t.last().unwrap().end_s() < EXIT_AT_S);
        assert!(t.iter().all(|c| (1.2..=2.0).contains(&c.duration_s)));
        let json = timeline_json();
        assert!(json.contains("\"tS\": 10.0") && json.contains("\"kind\": \"match_start\"") && json.contains("\"durationS\""));
    }

    #[test]
    fn boxes_fit_and_only_won_lost_share() {
        for a in ALL_BANNERS {
            let (x, y, w, h) = a.rect();
            assert!(x + w <= WIDTH && y + h <= HEIGHT);
            a.region().expanded(0.25).validate().unwrap();
            for b in ALL_BANNERS {
                if a == b || (matches!(a, Banner::RoundWin | Banner::RoundLoss) && matches!(b, Banner::RoundWin | Banner::RoundLoss)) {
                    continue;
                }
                let (bx, by, bw, bh) = b.rect();
                let overlap = x < bx + bw && bx < x + w && y < by + bh && by < y + h;
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn frames_change_and_banners_show() {
        let mut a = new_frame();
        let mut b = new_frame();
        render_frame(5.0, &mut a);
        render_frame(5.1, &mut b);
        assert_ne!(a, b, "background must animate");
        // Kill banner visible at 15.5 s: its bar is bright.
        render_frame(15.5, &mut a);
        let (x, y, _, _) = Banner::Kill.rect();
        let p = a[(y + 48) * WIDTH + x + 60];
        assert!((p >> 8) & 0xff > 200);
        assert!(in_match(15.5, &timeline()) && !in_match(35.0, &timeline()) && in_match(46.0, &timeline()));
    }
}
