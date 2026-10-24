//! Game and detector definitions, loaded from data files.
//!
//! Layout of a game folder (bundled under `games/` or user-added under
//! `%APPDATA%/Recap/games/`):
//!
//! ```text
//! games/valorant/
//!   game.json               # GameDef
//!   detectors/kill.json     # DetectorDef (one per file)
//!   templates/kill-1.png    # reference images referenced by detectors
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

/// Official local event sources a game can opt into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiSource {
    /// League of Legends / TFT Live Client Data API on 127.0.0.1:2999.
    LeagueLiveClient,
    /// Counter-Strike 2 Game State Integration.
    Cs2Gsi,
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
}

fn default_clip_events() -> Vec<String> {
    vec!["kill".into()]
}

impl Default for AutoClipDef {
    fn default() -> Self {
        Self {
            events: default_clip_events(),
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
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Optional notes for people sharing detector files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
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
        Ok(())
    }

    /// A detector can only run once it has at least one reference image.
    pub fn is_ready(&self) -> bool {
        self.enabled && !self.templates.is_empty()
    }
}

/// A game definition plus its detectors, with the folders they were loaded from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedGame {
    pub def: GameDef,
    pub detectors: Vec<LoadedDetector>,
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
        self.def.api == Some(ApiSource::Cs2Gsi) || self.detectors.iter().any(|d| crate::events::is_round_kind(&d.def.event))
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
    for t in &det.templates {
        let dst = user_game_dir.join(t);
        if !dst.exists() {
            let src = from_dir.join(t);
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
}
