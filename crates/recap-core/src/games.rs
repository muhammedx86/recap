//! Game and detector definitions, loaded from data files.
//!
//! Layout of a game folder (bundled under `games/` or user-added under
//! `%APPDATA%/Recap/games/`):
//!
//! ```text
//! games/valorant/
//!   game.json               # GameDef
//!   detectors/kill.json     # DetectorDef (one per file)
//!   readers/map.json        # ReaderDef: reads a match detail (map, K/D/A...) from the screen
//!   templates/kill-1.png    # reference images referenced by detectors and readers
//! ```
//!
//! A user folder with the same game id overrides the bundled one, detector by detector.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// A rectangle expressed as fractions (0..1) of the full frame, so it is
/// resolution independent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Region {
    pub fn validate(&self) -> Result<()> {
        let ok = |v: f32| (0.0..=1.0).contains(&v);
        if !(ok(self.x) && ok(self.y) && self.w > 0.0 && self.h > 0.0 && self.x + self.w <= 1.0001 && self.y + self.h <= 1.0001) {
            bail!("region {:?} must lie inside the frame (fractions 0..1)", self);
        }
        Ok(())
    }

    /// Pixel rectangle inside a `width`x`height` frame, clamped to the frame.
    pub fn to_pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let x = ((self.x * width as f32).round() as u32).min(width.saturating_sub(1));
        let y = ((self.y * height as f32).round() as u32).min(height.saturating_sub(1));
        let w = ((self.w * width as f32).round() as u32).max(1).min(width - x);
        let h = ((self.h * height as f32).round() as u32).max(1).min(height - y);
        (x, y, w, h)
    }

    /// This region, measured on a reference frame of aspect `ref_aspect` with the HUD element
    /// pinned to `anchor`, moved onto a `frame_w`x`frame_h` frame. The HUD is assumed to scale
    /// to fit (by height on wider screens, by width on narrower ones) and to stay pinned to the
    /// anchor, which is how most games lay out their HUD on ultrawide and 16:10 screens.
    /// Without an anchor the fractions are used as they are.
    pub fn place(&self, anchor: Option<Anchor>, ref_aspect: f32, frame_w: u32, frame_h: u32) -> Region {
        let Some(a) = anchor else { return *self };
        let aspect = frame_w.max(1) as f32 / frame_h.max(1) as f32;
        let s = hud_scale(ref_aspect, frame_w, frame_h);
        let (ax, ay) = a.point();
        // Work in units of the frame height: the reference frame is ref_aspect x 1.
        let fx = ax * aspect + (self.x * ref_aspect - ax * ref_aspect) * s;
        let fy = ay + (self.y - ay) * s;
        Region {
            x: fx / aspect,
            y: fy,
            w: self.w * ref_aspect * s / aspect,
            h: self.h * s,
        }
        .clamped()
    }

    /// The inverse of [`Region::place`]: a box drawn on a `frame_w`x`frame_h` frame, expressed on
    /// the reference frame.
    pub fn unplace(&self, anchor: Option<Anchor>, ref_aspect: f32, frame_w: u32, frame_h: u32) -> Region {
        let Some(a) = anchor else { return *self };
        let aspect = frame_w.max(1) as f32 / frame_h.max(1) as f32;
        let s = hud_scale(ref_aspect, frame_w, frame_h);
        let (ax, ay) = a.point();
        let rx = ax * ref_aspect + (self.x * aspect - ax * aspect) / s;
        Region {
            x: rx / ref_aspect,
            y: ay + (self.y - ay) / s,
            w: self.w * aspect / s / ref_aspect,
            h: self.h / s,
        }
        .clamped()
    }

    /// Clamp into the frame (keeping at least a sliver of size).
    pub fn clamped(&self) -> Region {
        let x = self.x.clamp(0.0, 0.999);
        let y = self.y.clamp(0.0, 0.999);
        Region {
            x,
            y,
            w: self.w.clamp(1e-3, 1.0 - x),
            h: self.h.clamp(1e-3, 1.0 - y),
        }
    }

    /// Grow the region by `margin` (fraction of its own size) on every side, clamped to the frame.
    pub fn expanded(&self, margin: f32) -> Region {
        let dx = self.w * margin;
        let dy = self.h * margin;
        let x = (self.x - dx).max(0.0);
        let y = (self.y - dy).max(0.0);
        let r = (self.x + self.w + dx).min(1.0);
        let b = (self.y + self.h + dy).min(1.0);
        Region { x, y, w: r - x, h: b - y }
    }
}

/// The point of the game picture a HUD element is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// (x, y) of the anchor as fractions of the frame.
    pub fn point(self) -> (f32, f32) {
        match self {
            Anchor::TopLeft => (0.0, 0.0),
            Anchor::Top => (0.5, 0.0),
            Anchor::TopRight => (1.0, 0.0),
            Anchor::Left => (0.0, 0.5),
            Anchor::Center => (0.5, 0.5),
            Anchor::Right => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 1.0),
            Anchor::Bottom => (0.5, 1.0),
            Anchor::BottomRight => (1.0, 1.0),
        }
    }
}

/// Aspect ratio anchored regions are measured on, unless a detector says otherwise.
pub const DEFAULT_ANCHOR_ASPECT: f32 = 16.0 / 9.0;

/// Size of the HUD on a `frame_w`x`frame_h` frame relative to the reference frame, in units of
/// the frame height: 1.0 on the reference aspect and on wider screens (the HUD scales with the
/// height), less on narrower ones (it scales with the width).
pub fn hud_scale(ref_aspect: f32, frame_w: u32, frame_h: u32) -> f32 {
    let aspect = frame_w.max(1) as f32 / frame_h.max(1) as f32;
    (aspect / ref_aspect.max(0.1)).min(1.0)
}

/// The frame height a crop cut from a `frame_w`x`frame_h` frame corresponds to on the anchor's
/// reference frame: anchored HUDs shrink on narrow frames, so their templates are stored at the
/// reference scale (`frame_h * hud_scale`). Unanchored definitions use the frame height as is.
pub fn reference_height(anchor: Option<Anchor>, ref_aspect: f32, frame_w: u32, frame_h: u32) -> u32 {
    match anchor {
        Some(_) => ((frame_h as f32 * hud_scale(ref_aspect, frame_w, frame_h)).round() as u32).max(1),
        None => frame_h,
    }
}

/// Optional HSV colour ranges: before matching, pixels inside any range become white and the rest
/// black, so only the HUD colour (say, a red kill icon) is compared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorFilter {
    pub ranges: Vec<HsvRange>,
}

/// Hue in degrees (0..360; `hue[0] > hue[1]` wraps through red), saturation and value 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HsvRange {
    pub hue: [f32; 2],
    pub sat: [f32; 2],
    pub val: [f32; 2],
}

impl HsvRange {
    pub fn contains(&self, h: f32, s: f32, v: f32) -> bool {
        let hue_ok = if self.hue[0] <= self.hue[1] {
            h >= self.hue[0] && h <= self.hue[1]
        } else {
            h >= self.hue[0] || h <= self.hue[1]
        };
        hue_ok && s >= self.sat[0] && s <= self.sat[1] && v >= self.val[0] && v <= self.val[1]
    }
}

impl ColorFilter {
    pub fn validate(&self) -> Result<()> {
        if self.ranges.is_empty() {
            bail!("a colour filter needs at least one range");
        }
        for r in &self.ranges {
            let ok01 = |a: [f32; 2]| (0.0..=1.0).contains(&a[0]) && (0.0..=1.0).contains(&a[1]) && a[0] <= a[1];
            if !(0.0..=360.0).contains(&r.hue[0]) || !(0.0..=360.0).contains(&r.hue[1]) || !ok01(r.sat) || !ok01(r.val) {
                bail!("colour range {r:?}: hue 0..360, saturation and value 0..1 (min <= max)");
            }
        }
        Ok(())
    }

    /// White where a pixel falls in one of the ranges, black elsewhere.
    pub fn apply(&self, img: &image::RgbImage) -> image::GrayImage {
        image::GrayImage::from_fn(img.width(), img.height(), |x, y| {
            let [r, g, b] = img.get_pixel(x, y).0;
            let (h, s, v) = rgb_to_hsv(r, g, b);
            image::Luma([if self.ranges.iter().any(|rg| rg.contains(h, s, v)) {
                255
            } else {
                0
            }])
        })
    }
}

/// (hue degrees 0..360, saturation 0..1, value 0..1)
pub fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

/// A detector whose references stand for values (digits on a kill counter, "2X"/"3X" multikill
/// badges): it reports the value of the best-matching reference and fires when that value
/// changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueRule {
    /// A value counts once `votes` of the last `window` samples agree on it.
    #[serde(default = "default_votes")]
    pub votes: u32,
    #[serde(default = "default_window")]
    pub window: u32,
    #[serde(default)]
    pub fire: ValueFire,
}

fn default_votes() -> u32 {
    3
}
fn default_window() -> u32 {
    5
}

/// When a value detector fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueFire {
    /// On any change of the settled value (not on the first one seen).
    #[default]
    Change,
    /// When a numeric value goes up: one event per step (a counter going 1 -> 3 is two kills).
    Increase,
}

/// Official local event sources a game can opt into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiSource {
    /// League of Legends / TFT Live Client Data API on 127.0.0.1:2999.
    LeagueLiveClient,
    /// Counter-Strike 2 Game State Integration.
    Cs2Gsi,
    /// VALORANT: the Riot Client's local API on 127.0.0.1 (map, mode, score, round results).
    RiotClient,
    /// Marvel Rivals: the game's own state file (`current_connect_info`) says when a match is on.
    MarvelStateFile,
}

impl ApiSource {
    /// Whether this source reports match start/end (so matches can be split on it).
    pub fn detects_matches(self) -> bool {
        matches!(self, ApiSource::LeagueLiveClient | ApiSource::Cs2Gsi | ApiSource::MarvelStateFile)
    }
}

/// How the recorder should capture this game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    /// OBS game capture hook (best quality, works in fullscreen).
    #[default]
    Game,
    /// Window capture (Windows Graphics Capture).
    Window,
    /// Whole monitor.
    Monitor,
}

/// Auto-clip defaults for a game (the user can override in settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoClipDef {
    /// Event kinds that produce clips.
    #[serde(default = "default_clip_events")]
    pub events: Vec<String>,
    /// Seconds kept before/after each kind of play in this game (over the built-in defaults).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub timing: std::collections::BTreeMap<String, ClipTiming>,
}

/// Seconds of video kept before and after a play when it is clipped.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClipTiming {
    pub before: f32,
    pub after: f32,
}

fn default_clip_events() -> Vec<String> {
    vec!["kill".into()]
}

impl Default for AutoClipDef {
    fn default() -> Self {
        Self {
            events: default_clip_events(),
            timing: Default::default(),
        }
    }
}

/// A post-match processor reference, e.g. an external tool that parses replay files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum PostMatchDef {
    /// Run a command after the match. It must print Recap events as JSON (see `postmatch`).
    /// `{recording}`, `{matchStartUnixMs}`, `{matchEndUnixMs}` and `{gameDir}` are substituted in args.
    Command {
        id: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        /// Folder the game writes its replay/demo files into (env vars like %LOCALAPPDATA% expand).
        #[serde(default)]
        watch_dir: Option<String>,
    },
}

/// `game.json`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDef {
    pub id: String,
    pub name: String,
    /// Executable names (case-insensitive) whose presence means the game is running.
    pub processes: Vec<String>,
    #[serde(default)]
    pub capture: CaptureMode,
    /// Optional official local API used *in addition to* vision detectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiSource>,
    /// When an API source is present, skip vision detectors for event kinds the API already covers.
    #[serde(default = "yes")]
    pub prefer_api: bool,
    #[serde(default)]
    pub auto_clip: AutoClipDef,
    #[serde(default)]
    pub post_match: Vec<PostMatchDef>,
    /// Accent colour for the UI (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Custom event types this game's detectors can emit (built-ins are always available).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_types: Vec<crate::events::EventTypeDef>,
    /// Round rules used to drop impossible screen detections (see [`crate::events::apply_round_rules`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<RoundRules>,
    /// Screen detections to drop when another event is close by (see [`crate::events::apply_exclusions`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<Exclusion>,
}

/// Drop vision events of kind `drop` from `beforeMs` before to `afterMs` after any event of kind
/// `near`. For a HUD
/// element that means one thing unless something else is on screen too (e.g. your own kill-feed
/// entry is a kill, unless the death screen is up: then it is your death).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exclusion {
    pub drop: String,
    pub near: String,
    #[serde(default)]
    pub before_ms: u64,
    #[serde(default)]
    pub after_ms: u64,
}

/// How rounds work in a round-based game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundRules {
    /// One life per round: after you die, kill banners on screen belong to the teammate you are
    /// spectating, and a second death in the same round is the same death shown again.
    #[serde(default)]
    pub one_life: bool,
    /// Nobody fights for this long after a round starts (buy phase / preparation). Death screens
    /// shown then are left over from the previous round.
    #[serde(default)]
    pub setup_ms: u64,
    /// The longest a round can go on after you die. Past this, a missed round start no longer
    /// hides later detections. 0 = until the next round boundary.
    #[serde(default)]
    pub max_after_death_ms: u64,
}

fn yes() -> bool {
    true
}

/// How a detector decides it saw something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetectMethod {
    /// Zero-mean normalized cross-correlation on brightness.
    #[default]
    Ncc,
    /// The same correlation on edge strength (outlines), so a HUD element matches whatever
    /// scene is behind it. Best for icons and banners drawn over gameplay.
    Edges,
}

/// A detector, one per file in `detectors/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectorDef {
    pub id: String,
    /// Human label, e.g. "Kill banner".
    #[serde(default)]
    pub label: String,
    /// Event kind emitted when this detector fires.
    pub event: String,
    /// Search area (fractions of the frame). Usually the template box plus a margin.
    pub region: Region,
    /// Where the reference image(s) were cut from, for display in Detector Studio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_box: Option<Region>,
    /// Reference images, relative to the game folder.
    #[serde(default)]
    pub templates: Vec<String>,
    /// Height in pixels of the frame the templates were cut from. Used to rescale
    /// templates to the analysed frame size.
    pub template_frame_height: u32,
    #[serde(default)]
    pub method: DetectMethod,
    /// Score (0..1) at or above which the detector counts as matching.
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    /// Minimum time between two events from this detector.
    #[serde(default = "default_cooldown")]
    pub cooldown_ms: u64,
    /// Consecutive matching samples required before firing (debounce).
    #[serde(default = "default_min_consecutive")]
    pub min_consecutive: u32,
    /// Samples per second when scanning.
    #[serde(default = "default_fps")]
    pub sample_fps: f32,
    /// Extra template scales to try, for HUD-scale differences (1.0 is always tried).
    #[serde(default)]
    pub scales: Vec<f32>,
    /// Re-fire while the element stays on screen when its score dips by at least this much and
    /// recovers (e.g. a kill banner that pulses on each of several quick kills). 0 = off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rearm_drop: f32,
    /// Once fired, the element counts as still on screen until its score falls this far below
    /// the threshold (hysteresis), so a flicker around the threshold is not a second play. 0 = off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hold_margin: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Pin `region` (and `templateBox`) to a point of the game picture, so the HUD element is found
    /// on ultrawide and 16:10 screens too. The region is then measured on a frame of
    /// `anchorAspect` (16:9 by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_aspect: Option<f32>,
    /// Compare only pixels in these colours (see [`ColorFilter`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_filter: Option<ColorFilter>,
    /// One value per reference image: the detector reports the value of the best match and
    /// fires when it changes (see [`ValueRule`]). Empty = an ordinary detector.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_rule: Option<ValueRule>,
    /// While this detector matches (spectating, a killcam, the scoreboard...), screen detections
    /// of these event kinds are dropped. A blocker marks nothing itself (its `event` is unused;
    /// by convention "_blocker").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    /// Extra time around a blocker's on-screen spans that is blocked too.
    #[serde(default = "default_block_margin", skip_serializing_if = "is_default_block_margin")]
    pub block_margin_ms: u64,
    /// Optional notes for people sharing detector files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn default_block_margin() -> u64 {
    1000
}
fn is_default_block_margin(v: &u64) -> bool {
    *v == default_block_margin()
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn default_threshold() -> f32 {
    0.8
}
fn default_cooldown() -> u64 {
    1500
}
fn default_min_consecutive() -> u32 {
    2
}
fn default_fps() -> f32 {
    6.0
}

impl DetectorDef {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!("detector id {:?} must be non-empty and use only letters, digits, - and _", self.id);
        }
        if self.event.trim().is_empty() {
            bail!("detector {} has no event kind", self.id);
        }
        self.region.validate().with_context(|| format!("detector {}", self.id))?;
        if !(0.0..=1.0).contains(&self.threshold) {
            bail!("detector {} threshold must be between 0 and 1", self.id);
        }
        if self.sample_fps <= 0.0 || self.sample_fps > 60.0 {
            bail!("detector {} sampleFps must be in (0, 60]", self.id);
        }
        if self.template_frame_height == 0 {
            bail!("detector {} templateFrameHeight must be > 0", self.id);
        }
        if let Some(a) = self.anchor_aspect {
            if !(0.5..=5.0).contains(&a) {
                bail!("detector {} anchorAspect must be between 0.5 and 5", self.id);
            }
        }
        if let Some(c) = &self.color_filter {
            c.validate().with_context(|| format!("detector {}", self.id))?;
        }
        if !self.values.is_empty() && self.values.len() != self.templates.len() {
            bail!(
                "detector {} needs one value per reference image ({} values, {} references)",
                self.id,
                self.values.len(),
                self.templates.len()
            );
        }
        if let Some(r) = &self.value_rule {
            if r.votes == 0 || r.window == 0 || r.votes > r.window || r.window > 60 {
                bail!("detector {} valueRule: 1 <= votes <= window <= 60", self.id);
            }
        }
        Ok(())
    }

    /// A detector can only run once it has at least one reference image.
    pub fn is_ready(&self) -> bool {
        self.enabled && !self.templates.is_empty()
    }

    /// Aspect ratio the region is measured on (anchored detectors).
    pub fn ref_aspect(&self) -> f32 {
        self.anchor_aspect.unwrap_or(DEFAULT_ANCHOR_ASPECT)
    }

    /// The search region on a `w`x`h` frame.
    pub fn region_for(&self, w: u32, h: u32) -> Region {
        self.region.place(self.anchor, self.ref_aspect(), w, h)
    }

    /// Whether this detector reports values (see [`ValueRule`]).
    pub fn is_value_detector(&self) -> bool {
        !self.values.is_empty()
    }
}

/// What a reader produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReaderKind {
    /// Text recognition (OCR) of the box, optionally filtered by a pattern or a list of values.
    #[default]
    Text,
    /// The best-matching labelled reference image (agent portraits, map names, VICTORY/DEFEAT).
    Choice,
}

/// How a reader settles on one value from many frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadPick {
    /// The value read most often.
    #[default]
    Vote,
    /// The last value read.
    Latest,
}

fn is_default_pick(p: &ReadPick) -> bool {
    *p == ReadPick::Vote
}

/// When in a match a reader looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadWhen {
    /// Around and after the end of the match (end screen, final scoreboard).
    #[default]
    End,
    /// The first minute (loading screen, agent select).
    Start,
    /// Throughout the match (persistent HUD such as your character's portrait).
    Any,
}

/// Repeated boxes, e.g. one card per player: slot `i` is `region` moved by `i * step`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slots {
    pub count: u32,
    #[serde(default)]
    pub step_x: f32,
    #[serde(default)]
    pub step_y: f32,
}

/// Pick the slot that shows a colour (your own row or card is usually highlighted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    /// Part of the slot to look at, as fractions of the slot.
    #[serde(rename = "box")]
    pub area: Region,
    /// Hex colour, e.g. "#ebe86a".
    pub color: String,
    /// Max per-channel difference (0..255) to count as that colour.
    #[serde(default = "default_tolerance")]
    pub tolerance: u8,
}

fn default_tolerance() -> u8 {
    48
}

/// One labelled value of a choice reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceOption {
    pub value: String,
    /// Reference images, relative to the game folder.
    #[serde(default)]
    pub templates: Vec<String>,
}

/// A screen reader, one per file in `readers/`: fills a match detail from what is on screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderDef {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// Detail field to fill (`map`, `character`, `kills`, ... or a custom id). Named groups in
    /// `pattern` (e.g. `(?P<kills>\d+)`) fill those fields instead.
    pub field: String,
    #[serde(default)]
    pub kind: ReaderKind,
    /// The box to read (the first slot when `slots` is set), as fractions of the game picture.
    pub region: Region,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<Slots>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<Highlight>,
    #[serde(default)]
    pub when: ReadWhen,
    /// Which reading wins when frames disagree: the most common one, or the last one seen (for
    /// values that change during a match, like a live K/D/A on the scoreboard).
    #[serde(default, skip_serializing_if = "is_default_pick")]
    pub pick: ReadPick,
    /// Frames per second to read while looking.
    #[serde(default = "default_reader_fps")]
    pub sample_fps: f32,
    /// Text: regular expression the reading must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Text: allowed values; readings snap to the closest one or are dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Choice: the labelled values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ChoiceOption>,
    /// Choice: minimum match score.
    #[serde(default = "default_choice_threshold")]
    pub threshold: f32,
    #[serde(default)]
    pub method: DetectMethod,
    /// Height of the frame the choice references were cut from.
    #[serde(default = "default_frame_height")]
    pub template_frame_height: u32,
    /// Pin the box to a point of the game picture (see [`DetectorDef::anchor`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_aspect: Option<f32>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

fn default_reader_fps() -> f32 {
    1.0
}
fn default_choice_threshold() -> f32 {
    0.75
}
fn default_frame_height() -> u32 {
    1080
}

impl ReaderDef {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!("reader id {:?} must be non-empty and use only letters, digits, - and _", self.id);
        }
        if self.field.trim().is_empty() {
            bail!("reader {} has no field", self.id);
        }
        self.region.validate().with_context(|| format!("reader {}", self.id))?;
        if let Some(p) = &self.pattern {
            regex::Regex::new(p).with_context(|| format!("reader {} pattern", self.id))?;
        }
        if let Some(sl) = &self.slots {
            if sl.count == 0 || sl.count > 16 {
                bail!("reader {} slots.count must be 1..16", self.id);
            }
        }
        if let Some(h) = &self.highlight {
            parse_hex(&h.color).with_context(|| format!("reader {} highlight colour", self.id))?;
        }
        if self.sample_fps <= 0.0 || self.sample_fps > 10.0 {
            bail!("reader {} sampleFps must be in (0, 10]", self.id);
        }
        Ok(())
    }

    /// Text readers always run; choice readers need at least one labelled reference.
    pub fn is_ready(&self) -> bool {
        self.enabled && (self.kind == ReaderKind::Text || self.options.iter().any(|o| !o.templates.is_empty()))
    }

    /// Box of slot `i`.
    pub fn slot_region(&self, i: u32) -> Region {
        let (dx, dy) = self.slots.map(|s| (s.step_x * i as f32, s.step_y * i as f32)).unwrap_or((0.0, 0.0));
        let x = (self.region.x + dx).clamp(0.0, 1.0 - self.region.w);
        let y = (self.region.y + dy).clamp(0.0, 1.0 - self.region.h);
        Region { x, y, ..self.region }
    }

    /// Box of slot `i` on a `w`x`h` frame (anchored readers move with the HUD).
    pub fn slot_region_for(&self, i: u32, w: u32, h: u32) -> Region {
        self.slot_region(i)
            .place(self.anchor, self.anchor_aspect.unwrap_or(DEFAULT_ANCHOR_ASPECT), w, h)
    }

    pub fn slot_count(&self) -> u32 {
        self.slots.map(|s| s.count).unwrap_or(1).max(1)
    }
}

/// "#rrggbb" -> [r, g, b]
pub fn parse_hex(s: &str) -> Result<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        bail!("colour {s:?} must look like #rrggbb");
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).with_context(|| format!("colour {s:?}"));
    Ok([p(0)?, p(2)?, p(4)?])
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedReader {
    pub def: ReaderDef,
    /// Game folder the template paths are relative to.
    pub base_dir: PathBuf,
    pub user: bool,
}

/// A game definition plus its detectors, with the folders they were loaded from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedGame {
    pub def: GameDef,
    pub detectors: Vec<LoadedDetector>,
    pub readers: Vec<LoadedReader>,
    /// Folder holding game.json (user folder if overridden).
    pub dir: PathBuf,
    /// True if any file comes from the user's folder.
    pub user_modified: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedDetector {
    pub def: DetectorDef,
    /// Game folder the template paths are relative to.
    pub base_dir: PathBuf,
    pub user: bool,
}

impl LoadedDetector {
    pub fn template_paths(&self) -> Vec<PathBuf> {
        self.def.templates.iter().map(|t| self.base_dir.join(t)).collect()
    }
}

impl LoadedGame {
    /// Built-in event types followed by this game's custom ones.
    pub fn event_types(&self) -> Vec<crate::events::EventTypeDef> {
        let mut v = crate::events::builtin_event_types();
        for t in &self.def.event_types {
            if !v.iter().any(|b| b.id == t.id) {
                v.push(t.clone());
            }
        }
        v
    }

    /// True when the game produces round markers (round detectors or CS2 GSI).
    pub fn has_rounds(&self) -> bool {
        matches!(self.def.api, Some(ApiSource::Cs2Gsi | ApiSource::RiotClient))
            || self.detectors.iter().any(|d| crate::events::is_round_kind(&d.def.event))
    }

    pub fn ready_detectors(&self) -> impl Iterator<Item = &LoadedDetector> {
        self.detectors.iter().filter(|d| d.def.is_ready())
    }

    pub fn matches_process(&self, exe_name: &str) -> bool {
        self.def.processes.iter().any(|p| p.eq_ignore_ascii_case(exe_name))
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn load_readers(game_dir: &Path, user: bool) -> Vec<LoadedReader> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(game_dir.join("readers")) else {
        return out;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for p in paths {
        match read_json::<ReaderDef>(&p).and_then(|d| d.validate().map(|_| d)) {
            Ok(def) => out.push(LoadedReader {
                def,
                base_dir: game_dir.to_path_buf(),
                user,
            }),
            Err(e) => log::warn!("skipping reader {}: {e:#}", p.display()),
        }
    }
    out
}

fn load_detectors(game_dir: &Path, user: bool) -> Vec<LoadedDetector> {
    let mut out = Vec::new();
    let dir = game_dir.join("detectors");
    let Ok(entries) = fs::read_dir(&dir) else { return out };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for p in paths {
        match read_json::<DetectorDef>(&p).and_then(|d| d.validate().map(|_| d)) {
            Ok(def) => out.push(LoadedDetector {
                def,
                base_dir: game_dir.to_path_buf(),
                user,
            }),
            Err(e) => log::warn!("skipping detector {}: {e:#}", p.display()),
        }
    }
    out
}

/// Load games from several roots, later roots overriding earlier ones (bundled first, user last).
pub fn load_games(roots: &[PathBuf]) -> Vec<LoadedGame> {
    let mut games: BTreeMap<String, LoadedGame> = BTreeMap::new();
    for (i, root) in roots.iter().enumerate() {
        let user = i > 0;
        let Ok(entries) = fs::read_dir(root) else { continue };
        let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let game_json = dir.join("game.json");
            let def = if game_json.exists() {
                match read_json::<GameDef>(&game_json) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        log::warn!("skipping game {}: {e:#}", dir.display());
                        continue;
                    }
                }
            } else {
                None
            };
            let detectors = load_detectors(&dir, user);
            let readers = load_readers(&dir, user);
            let id = def
                .as_ref()
                .map(|d| d.id.clone())
                .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
            match games.get_mut(&id) {
                Some(existing) => {
                    if let Some(def) = def {
                        existing.def = def;
                        existing.dir = dir.clone();
                    }
                    for d in detectors {
                        existing.detectors.retain(|x| x.def.id != d.def.id);
                        existing.detectors.push(d);
                    }
                    for r in readers {
                        existing.readers.retain(|x| x.def.id != r.def.id);
                        existing.readers.push(r);
                    }
                    existing.user_modified |= user;
                }
                None => {
                    let Some(def) = def else {
                        log::warn!("{} has detectors but no game.json; ignoring", dir.display());
                        continue;
                    };
                    games.insert(
                        id,
                        LoadedGame {
                            def,
                            detectors,
                            readers,
                            dir,
                            user_modified: user,
                        },
                    );
                }
            }
        }
    }
    let mut v: Vec<_> = games.into_values().collect();
    for g in &mut v {
        g.detectors.sort_by(|a, b| a.def.id.cmp(&b.def.id));
        g.readers.sort_by(|a, b| a.def.id.cmp(&b.def.id));
    }
    v.sort_by_key(|g| g.def.name.to_lowercase());
    v
}

/// Write a detector (and optionally a new template PNG) into the user's game folder.
/// Returns the path of the detector file.
pub fn save_user_detector(user_root: &Path, game: &GameDef, det: &DetectorDef, new_template: Option<(&str, &[u8])>) -> Result<PathBuf> {
    det.validate()?;
    let game_dir = user_root.join(&game.id);
    fs::create_dir_all(game_dir.join("detectors"))?;
    let gj = game_dir.join("game.json");
    if !gj.exists() {
        fs::write(&gj, serde_json::to_string_pretty(game)?)?;
    }
    if let Some((name, bytes)) = new_template {
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            bail!("invalid template file name {name:?}");
        }
        fs::create_dir_all(game_dir.join("templates"))?;
        fs::write(game_dir.join("templates").join(name), bytes)?;
    }
    // Templates referenced from the bundled folder must exist in the user folder too.
    let path = game_dir.join("detectors").join(format!("{}.json", det.id));
    fs::write(&path, serde_json::to_string_pretty(det)?)?;
    Ok(path)
}

/// Copy any template files a detector references from `from_dir` into the user game folder if missing.
pub fn ensure_templates_copied(det: &DetectorDef, from_dir: &Path, user_game_dir: &Path) -> Result<()> {
    // Each reference, plus its hand-made mask when there is one.
    let files = det.templates.iter().flat_map(|t| {
        let mask = crate::vision::mask_path(Path::new(t)).map(|m| m.to_string_lossy().into_owned());
        std::iter::once(t.clone()).chain(mask)
    });
    for t in files {
        let dst = user_game_dir.join(&t);
        if !dst.exists() {
            let src = from_dir.join(&t);
            if src.exists() {
                if let Some(p) = dst.parent() {
                    fs::create_dir_all(p)?;
                }
                fs::copy(&src, &dst)?;
            }
        }
    }
    Ok(())
}

/// Write a reader (and optionally new reference PNGs, as (file name, bytes)) into the user's game folder.
pub fn save_user_reader(user_root: &Path, game: &GameDef, reader: &ReaderDef, new_templates: &[(String, Vec<u8>)]) -> Result<PathBuf> {
    reader.validate()?;
    let game_dir = user_root.join(&game.id);
    fs::create_dir_all(game_dir.join("readers"))?;
    let gj = game_dir.join("game.json");
    if !gj.exists() {
        fs::write(&gj, serde_json::to_string_pretty(game)?)?;
    }
    for (name, bytes) in new_templates {
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            bail!("invalid template file name {name:?}");
        }
        fs::create_dir_all(game_dir.join("templates"))?;
        fs::write(game_dir.join("templates").join(name), bytes)?;
    }
    let path = game_dir.join("readers").join(format!("{}.json", reader.id));
    fs::write(&path, serde_json::to_string_pretty(reader)?)?;
    Ok(path)
}

pub fn delete_user_reader(user_root: &Path, game_id: &str, reader_id: &str) -> Result<()> {
    let p = user_root.join(game_id).join("readers").join(format!("{reader_id}.json"));
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

pub fn delete_user_detector(user_root: &Path, game_id: &str, detector_id: &str) -> Result<()> {
    let p = user_root.join(game_id).join("detectors").join(format!("{detector_id}.json"));
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Create or edit a custom event type on a game (written to the user game folder).
/// Returns the updated game definition.
pub fn upsert_event_type(user_root: &Path, game: &GameDef, mut t: crate::events::EventTypeDef) -> Result<GameDef> {
    use crate::events::{builtin_event_types, slugify, AUTO_COLORS};
    t.label = t.label.trim().to_string();
    if t.id.trim().is_empty() {
        t.id = slugify(&t.label);
    }
    if t.color.trim().is_empty() {
        let n = game.event_types.len();
        t.color = AUTO_COLORS[n % AUTO_COLORS.len()].to_string();
    }
    t.builtin = false;
    t.validate()?;
    if builtin_event_types().iter().any(|b| b.id == t.id) {
        bail!("\"{}\" is a built-in event type; pick another name", t.label);
    }
    let mut g = game.clone();
    match g.event_types.iter_mut().find(|x| x.id == t.id) {
        Some(existing) => *existing = t,
        None => g.event_types.push(t),
    }
    save_user_game(user_root, &g)?;
    Ok(g)
}

/// Remove a custom event type unless a detector still emits it.
pub fn remove_event_type(user_root: &Path, game: &LoadedGame, type_id: &str) -> Result<GameDef> {
    if let Some(d) = game.detectors.iter().find(|d| d.def.event == type_id) {
        bail!(
            "detector \"{}\" still marks this event type; change or delete it first",
            if d.def.label.is_empty() { &d.def.id } else { &d.def.label }
        );
    }
    let mut g = game.def.clone();
    let before = g.event_types.len();
    g.event_types.retain(|t| t.id != type_id);
    if g.event_types.len() == before {
        bail!("no custom event type {type_id:?} on {}", g.name);
    }
    save_user_game(user_root, &g)?;
    Ok(g)
}

/// Save (or overwrite) a user game definition.
pub fn save_user_game(user_root: &Path, game: &GameDef) -> Result<PathBuf> {
    if game.id.trim().is_empty() || !game.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("game id {:?} must use only letters, digits, - and _", game.id);
    }
    if game.processes.is_empty() {
        bail!("game {} needs at least one process name", game.id);
    }
    let dir = user_root.join(&game.id);
    fs::create_dir_all(&dir)?;
    let p = dir.join("game.json");
    fs::write(&p, serde_json::to_string_pretty(game)?)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(id: &str) -> DetectorDef {
        DetectorDef {
            id: id.into(),
            label: "Kill".into(),
            event: "kill".into(),
            region: Region {
                x: 0.4,
                y: 0.7,
                w: 0.2,
                h: 0.1,
            },
            template_box: None,
            templates: vec![],
            template_frame_height: 1080,
            method: DetectMethod::Ncc,
            threshold: 0.8,
            cooldown_ms: 1000,
            min_consecutive: 2,
            sample_fps: 6.0,
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

    fn game(id: &str) -> GameDef {
        GameDef {
            id: id.into(),
            name: id.to_uppercase(),
            processes: vec![format!("{id}.exe")],
            capture: CaptureMode::Game,
            api: None,
            prefer_api: true,
            auto_clip: AutoClipDef::default(),
            post_match: vec![],
            color: None,
            event_types: vec![],
            rounds: None,
            exclusions: vec![],
        }
    }

    #[test]
    fn region_pixels_and_expand() {
        let r = Region {
            x: 0.5,
            y: 0.5,
            w: 0.25,
            h: 0.25,
        };
        assert_eq!(r.to_pixels(1920, 1080), (960, 540, 480, 270));
        let e = Region {
            x: 0.0,
            y: 0.0,
            w: 0.1,
            h: 0.1,
        }
        .expanded(0.5);
        assert_eq!(e.x, 0.0);
        assert!((e.w - 0.15).abs() < 1e-6);
        assert!(Region {
            x: 0.9,
            y: 0.0,
            w: 0.2,
            h: 0.1
        }
        .validate()
        .is_err());
    }

    #[test]
    fn user_folder_overrides_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        let user = tmp.path().join("user");
        fs::create_dir_all(bundled.join("val/detectors")).unwrap();
        fs::write(bundled.join("val/game.json"), serde_json::to_string(&game("val")).unwrap()).unwrap();
        fs::write(
            bundled.join("val/detectors/kill.json"),
            serde_json::to_string(&det("kill")).unwrap(),
        )
        .unwrap();
        fs::write(bundled.join("val/detectors/broken.json"), "{not json").unwrap();

        let mut d2 = det("kill");
        d2.threshold = 0.9;
        save_user_detector(&user, &game("val"), &d2, Some(("k.png", b"png"))).unwrap();

        let games = load_games(&[bundled, user.clone()]);
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(g.detectors.len(), 1, "broken detector skipped, user one replaces bundled");
        assert_eq!(g.detectors[0].def.threshold, 0.9);
        assert!(g.detectors[0].user);
        assert!(g.user_modified);
        assert!(user.join("val/templates/k.png").exists());
        assert!(g.matches_process("VAL.EXE"));
    }

    #[test]
    fn custom_event_types() {
        use crate::events::EventTypeDef;
        let tmp = tempfile::tempdir().unwrap();
        let t = |label: &str, icon: &str| EventTypeDef {
            id: String::new(),
            label: label.into(),
            icon: icon.into(),
            color: String::new(),
            builtin: false,
        };
        let g = upsert_event_type(tmp.path(), &game("val"), t("Ult used", "zap")).unwrap();
        assert_eq!(g.event_types[0].id, "ult_used");
        assert!(g.event_types[0].color.starts_with('#'));
        assert!(upsert_event_type(tmp.path(), &g, t("Kill", "skull")).is_err(), "built-in id");
        assert!(upsert_event_type(tmp.path(), &g, t("Bad", "sword")).is_err(), "unknown icon");
        // Edit keeps one entry.
        let mut e = g.event_types[0].clone();
        e.icon = "flame".into();
        let g = upsert_event_type(tmp.path(), &g, e).unwrap();
        assert_eq!(g.event_types.len(), 1);
        assert_eq!(g.event_types[0].icon, "flame");

        let mut d = det("ult");
        d.event = "ult_used".into();
        save_user_detector(tmp.path(), &g, &d, None).unwrap();
        let loaded = load_games(&[tmp.path().to_path_buf()]);
        assert_eq!(loaded[0].event_types().len(), 16);
        assert!(remove_event_type(tmp.path(), &loaded[0], "ult_used").is_err(), "still used");
        delete_user_detector(tmp.path(), "val", "ult").unwrap();
        let loaded = load_games(&[tmp.path().to_path_buf()]);
        assert!(remove_event_type(tmp.path(), &loaded[0], "ult_used")
            .unwrap()
            .event_types
            .is_empty());
        assert!(!loaded[0].has_rounds());
    }

    #[test]
    fn rejects_bad_ids() {
        assert!(det("../x").validate().is_err());
        let tmp = tempfile::tempdir().unwrap();
        assert!(save_user_detector(tmp.path(), &game("g"), &det("ok"), Some(("../evil.png", b""))).is_err());
    }

    #[test]
    fn anchored_regions_follow_the_hud_on_other_aspects() {
        use super::{Anchor, Region, DEFAULT_ANCHOR_ASPECT as R};
        // A 220x220 box centred at (959, 863) on 1920x1080, pinned to the bottom centre.
        let r = Region {
            x: 849.0 / 1920.0,
            y: 753.0 / 1080.0,
            w: 220.0 / 1920.0,
            h: 220.0 / 1080.0,
        };
        // Same aspect: unchanged.
        let same = r.place(Some(Anchor::Bottom), R, 1280, 720);
        assert!((same.x - r.x).abs() < 1e-4 && (same.w - r.w).abs() < 1e-4);
        // Ultrawide 2592x1080: same pixels, centred.
        let (x, y, w, h) = r.place(Some(Anchor::Bottom), R, 2592, 1080).to_pixels(2592, 1080);
        assert_eq!((w, h), (220, 220));
        assert!((x as i32 - (1296 - 111)).abs() <= 2 && y == 753, "{x} {y}");
        // 16:10 1920x1200: the HUD fits the width, stays pinned to the bottom.
        let (x, y, w, h) = r.place(Some(Anchor::Bottom), R, 1920, 1200).to_pixels(1920, 1200);
        assert_eq!((x, w, h), (849, 220, 220));
        assert_eq!(y, 1200 - (1080 - 753));
        // Top-right pinned elements stay against the right edge on ultrawide.
        let tr = Region {
            x: 0.8,
            y: 0.1,
            w: 0.15,
            h: 0.1,
        };
        let p = tr.place(Some(Anchor::TopRight), R, 2592, 1080);
        assert!((1.0 - (p.x + p.w) - 0.05 * 1920.0 / 2592.0).abs() < 1e-3);
        // Round trip.
        for (fw, fh) in [(2592, 1080), (1920, 1200), (1280, 720)] {
            for a in [Anchor::TopLeft, Anchor::Center, Anchor::BottomRight] {
                let back = r.place(Some(a), R, fw, fh).unplace(Some(a), R, fw, fh);
                assert!(
                    (back.x - r.x).abs() < 1e-3 && (back.y - r.y).abs() < 1e-3 && (back.w - r.w).abs() < 1e-3,
                    "{a:?} {fw}x{fh}"
                );
            }
        }
        // No anchor: fractions as they are.
        assert_eq!(r.place(None, R, 2592, 1080), r);
    }

    #[test]
    fn color_filter_keeps_only_the_hud_colour() {
        use super::{rgb_to_hsv, ColorFilter, HsvRange};
        let (h, s, v) = rgb_to_hsv(255, 0, 0);
        assert!((h - 0.0).abs() < 1e-3 && (s - 1.0).abs() < 1e-3 && (v - 1.0).abs() < 1e-3);
        let (h, _, _) = rgb_to_hsv(0, 0, 255);
        assert!((h - 240.0).abs() < 1e-3);
        // Reds wrap through 0 degrees.
        let red = ColorFilter {
            ranges: vec![HsvRange {
                hue: [340.0, 20.0],
                sat: [0.5, 1.0],
                val: [0.4, 1.0],
            }],
        };
        red.validate().unwrap();
        let img = image::RgbImage::from_fn(3, 1, |x, _| image::Rgb([[230, 30, 40], [40, 200, 60], [250, 250, 250]][x as usize]));
        let m = red.apply(&img);
        assert_eq!(m.as_raw(), &vec![255, 0, 0]);
        assert!(ColorFilter { ranges: vec![] }.validate().is_err());
    }
}
