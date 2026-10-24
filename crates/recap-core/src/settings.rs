//! User settings, stored as JSON in the app config folder.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where recordings and clips are written. Empty = `<Videos>/Recap`.
    pub recordings_dir: String,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// None = native resolution.
    pub output_height: Option<u32>,
    /// "auto" | "nvenc" | "amf" | "qsv" | "x264"
    pub encoder: String,
    pub record_desktop_audio: bool,
    pub record_microphone: bool,
    pub microphone_device: Option<String>,
    /// Also keep the microphone alone on audio track 2 (track 1 always has everything mixed, so
    /// players and clips sound right; editors can use track 2 to adjust or mute your voice).
    pub separate_mic_track: bool,
    /// Start Recap when Windows starts.
    pub launch_at_startup: bool,
    /// When started with Windows, stay in the tray instead of opening the window.
    pub start_minimized: bool,
    /// Small toasts over the game (recording started/saved, bookmark, clip saved, skipped match).
    pub notifications: bool,
    /// "top-right" | "top-left" | "bottom-right" | "bottom-left"
    pub notification_corner: String,
    /// Global hotkey that drops a bookmark on the current recording.
    pub bookmark_hotkey: String,
    /// Global hotkey that starts/stops a manual recording.
    pub manual_record_hotkey: String,
    pub auto_clip: AutoClipSettings,
    /// Run vision detectors over each recording when the match ends.
    pub scan_after_match: bool,
    /// Height frames are scaled to for detection.
    pub analysis_height: u32,
    /// Per-game on/off (game id -> enabled). Missing = enabled.
    pub games_enabled: BTreeMap<String, bool>,
    /// Keep at most this many GB of recordings (oldest non-favourite deleted first). None = unlimited.
    pub storage_limit_gb: Option<f32>,
    /// Include bookmarks in the in-game toasts.
    pub notify_on_bookmark: bool,
    /// Split a game session into one recording per match using match_start/match_end markers.
    pub split_matches: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoClipSettings {
    pub enabled: bool,
    pub seconds_before: f32,
    pub seconds_after: f32,
    /// Merge clips whose windows are closer than this.
    pub merge_gap_seconds: f32,
    /// Event kinds that make clips, overriding each game's default when non-empty.
    pub events_override: Vec<String>,
    /// Re-encode clips for frame-accurate cuts (slower). Off = stream copy (instant, keyframe-aligned).
    pub precise_cuts: bool,
}

impl Default for AutoClipSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            seconds_before: 5.0,
            seconds_after: 3.0,
            merge_gap_seconds: 2.0,
            events_override: vec![],
            precise_cuts: false,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recordings_dir: String::new(),
            fps: 60,
            bitrate_kbps: 12_000,
            output_height: None,
            encoder: "auto".into(),
            record_desktop_audio: true,
            record_microphone: false,
            microphone_device: None,
            separate_mic_track: true,
            launch_at_startup: false,
            start_minimized: true,
            notifications: true,
            notification_corner: "top-right".into(),
            bookmark_hotkey: "F8".into(),
            manual_record_hotkey: "F9".into(),
            auto_clip: AutoClipSettings::default(),
            scan_after_match: true,
            analysis_height: 720,
            games_enabled: BTreeMap::new(),
            storage_limit_gb: None,
            notify_on_bookmark: true,
            split_matches: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn game_enabled(&self, id: &str) -> bool {
        self.games_enabled.get(id).copied().unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"fps":30,"autoClip":{"secondsBefore":8}}"#).unwrap();
        assert_eq!(s.fps, 30);
        assert_eq!(s.auto_clip.seconds_before, 8.0);
        assert_eq!(s.auto_clip.seconds_after, 3.0);
        assert_eq!(s.bookmark_hotkey, "F8");
    }

    #[test]
    fn save_and_load() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut s = Settings::default();
        s.games_enabled.insert("cs2".into(), false);
        s.save(&p).unwrap();
        let l = Settings::load(&p);
        assert!(!l.game_enabled("cs2"));
        assert!(l.game_enabled("valorant"));
    }
}
