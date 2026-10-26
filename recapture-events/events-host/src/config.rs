//! `events\config.yaml`: what each event kind does, per game.
//!
//! ```yaml
//! enabled: true
//! games:
//!   valorant:
//!     events:
//!       kill:      { action: clip, before: 10, after: 3 }
//!       death:     { action: marker }
//!       match_end: { action: ignore }
//! ```
//!
//! A kind not listed falls back to the game's data: kinds in `game.json`'s `autoClip.events` are
//! clips (with Recap's timing), everything else is a marker. A marker's window is sent too, flagged as not asked
//! for: the engine's `[games] auto_clips` setting picks which windows are saved.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use recap_core::clips::default_timing;
use recap_core::games::GameDef;
use serde::{Deserialize, Serialize};

/// Seconds around an event when a clip has no timing of its own.
const FALLBACK_TIMING: (f32, f32) = (6.0, 3.0);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub games: BTreeMap<String, GameConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameConfig {
    #[serde(default)]
    pub events: BTreeMap<String, EventConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventConfig {
    pub action: Action,
    /// Seconds kept before / after the event (clips only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Clip,
    Marker,
    Ignore,
}

/// What to do with one event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resolved {
    /// Save `before_ms` before to `after_ms` after it.
    Clip { before_ms: u64, after_ms: u64 },
    /// A marker; its window (the kind's clip timing) is still sent, for the engine's "big moments".
    Marker { before_ms: u64, after_ms: u64 },
    Ignore,
}

fn yes() -> bool {
    true
}

impl Config {
    /// Reads the file; a missing file is the default config (enabled, everything from game data).
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self { enabled: true, ..Self::default() }),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn resolve(&self, game: &GameDef, kind: &str) -> Resolved {
        let set = self.games.get(&game.id).and_then(|g| g.events.get(kind));
        let action = match set {
            Some(e) => e.action,
            None if game.auto_clip.events.iter().any(|k| k == kind) => Action::Clip,
            None => Action::Marker,
        };
        let (b, a) = game
            .auto_clip
            .timing
            .get(kind)
            .map(|t| (t.before, t.after))
            .or_else(|| default_timing(kind))
            .unwrap_or(FALLBACK_TIMING);
        let before_ms = secs_to_ms(set.and_then(|e| e.before).unwrap_or(b));
        let after_ms = secs_to_ms(set.and_then(|e| e.after).unwrap_or(a));
        match action {
            Action::Ignore => Resolved::Ignore,
            Action::Marker => Resolved::Marker { before_ms, after_ms },
            Action::Clip => Resolved::Clip { before_ms, after_ms },
        }
    }
}

fn secs_to_ms(s: f32) -> u64 {
    (s.max(0.0) * 1000.0).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> GameDef {
        serde_json::from_value(serde_json::json!({
            "id": "cs2", "name": "Counter-Strike 2", "processes": ["cs2.exe"],
            "autoClip": { "events": ["kill"] }
        }))
        .unwrap()
    }

    #[test]
    fn unlisted_kinds_follow_the_game_data() {
        let c = Config::default();
        assert_eq!(c.resolve(&game(), "kill"), Resolved::Clip { before_ms: 6000, after_ms: 3000 });
        assert_eq!(c.resolve(&game(), "death"), Resolved::Marker { before_ms: 7000, after_ms: 2000 });
    }

    #[test]
    fn yaml_overrides_action_and_timing() {
        let c: Config = serde_yaml::from_str(
            "games:\n  cs2:\n    events:\n      kill: { action: clip, before: 10 }\n      death: { action: clip }\n      mvp: { action: ignore }\n",
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.resolve(&game(), "kill"), Resolved::Clip { before_ms: 10_000, after_ms: 3000 });
        assert_eq!(c.resolve(&game(), "death"), Resolved::Clip { before_ms: 7000, after_ms: 2000 });
        assert_eq!(c.resolve(&game(), "mvp"), Resolved::Ignore);
    }

    #[test]
    fn missing_file_is_enabled_default() {
        let c = Config::load(Path::new("does/not/exist.yaml")).unwrap();
        assert!(c.enabled && c.games.is_empty());
    }
}

#[cfg(test)]
mod shipped {
    use super::*;

    #[test]
    fn the_shipped_default_config_parses() {
        let text = include_str!("../../config.default.yaml");
        let c: Config = serde_yaml::from_str(text).unwrap();
        assert!(c.enabled);
        assert_eq!(c.games["cs2"].events["kill"].action, Action::Clip);
    }
}
