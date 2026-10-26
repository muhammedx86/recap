//! `--scan <clip>`: a saved clip's picture through the game's detectors (Recap's vision scan), for games whose kills
//! and deaths are only on screen. Prints one JSON line per event to stdout, at times from the clip's start:
//!
//! ```json
//! {"v":1,"type":"found","kind":"kill","tMs":6123,"source":"vision:kill-banner","confidence":0.93}
//! ```
//!
//! The engine adds them to the clip's `.events.json`. Kinds the game's live source already reports (CS2's kills
//! from GSI, VALORANT's round results) are left out, so nothing is marked twice.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use recap_core::events::{apply_exclusions, apply_round_rules, GameEvent};
use recap_core::ffmpeg::Ffmpeg;
use recap_core::games::{ApiSource, LoadedGame};
use recap_core::scan::{scan_video, ScanOptions};

use crate::protocol::VERSION;

/// The kinds a live source sends while playing: a scan doesn't repeat them.
fn live_kinds(api: Option<ApiSource>) -> &'static [&'static str] {
    match api {
        Some(ApiSource::Cs2Gsi) => &[
            "kill", "death", "assist", "mvp", "bomb_plant", "bomb_defuse", "round_start", "round_win", "round_loss",
            "match_start", "match_end",
        ],
        Some(ApiSource::LeagueLiveClient) => &["kill", "death", "assist", "multikill", "ace", "objective", "match_end"],
        Some(ApiSource::RiotClient) => &["round_win", "round_loss"],
        Some(ApiSource::RocketLeagueStats) => &[
            "goal", "goal_against", "assist", "save", "epic_save", "shot", "demo", "demoed", "mvp", "match_start",
            "match_end",
        ],
        // the state file only says when a match starts and ends, which the engine isn't sent
        Some(ApiSource::MarvelStateFile) | None => &[],
    }
}

/// The events of `clip` the game's live source doesn't already give, in time order.
pub fn events(game: &LoadedGame, clip: &Path, ffmpeg: &[PathBuf]) -> Result<Vec<GameEvent>> {
    if game.detectors.is_empty() {
        return Ok(Vec::new());
    }
    let res = scan_video(&Ffmpeg::locate(ffmpeg), clip, &game.detectors, ScanOptions::default())?;
    let kept = match &game.def.rounds {
        Some(r) => apply_round_rules(res.events, r),
        None => res.events,
    };
    let live = live_kinds(game.def.api);
    let mut kept: Vec<GameEvent> = apply_exclusions(kept, &game.def.exclusions)
        .into_iter()
        .filter(|e| !e.kind.starts_with('_') && !live.contains(&e.kind.as_str()))
        .collect();
    kept.sort_by_key(|e| e.t_ms);
    Ok(kept)
}

/// One output line for an event.
pub fn line(e: &GameEvent) -> String {
    let mut v = serde_json::json!({
        "v": VERSION,
        "type": "found",
        "kind": e.kind,
        "tMs": e.t_ms,
        "source": e.source.label(),
    });
    if let Some(c) = e.confidence {
        v["confidence"] = c.into();
    }
    let mut s = v.to_string();
    s.push('\n');
    s
}

/// Scans `clip` and prints what it found.
pub fn run(game: &LoadedGame, clip: &Path, ffmpeg: &[PathBuf]) -> Result<()> {
    let found = events(game, clip, ffmpeg)?;
    log::info!("{}: {} event(s) found in {}", game.def.name, found.len(), clip.display());
    let mut out = std::io::stdout().lock();
    for e in &found {
        out.write_all(line(e).as_bytes())?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use recap_core::events::EventSource;

    #[test]
    fn a_found_event_is_one_line_from_the_clip_start() {
        let mut e = GameEvent::new("kill", 6123, EventSource::Vision { detector: "kill-banner".into() });
        e.confidence = Some(0.5);
        let v: serde_json::Value = serde_json::from_str(line(&e).trim()).unwrap();
        assert_eq!(v["type"], "found");
        assert_eq!(v["kind"], "kill");
        assert_eq!(v["tMs"], 6123);
        assert_eq!(v["source"], "vision:kill-banner");
        assert_eq!(v["confidence"], 0.5);
    }

    #[test]
    fn live_kinds_are_not_scanned_twice() {
        assert!(live_kinds(Some(ApiSource::Cs2Gsi)).contains(&"kill"));
        assert!(!live_kinds(Some(ApiSource::RiotClient)).contains(&"kill"), "VALORANT kills come from the screen");
        assert!(live_kinds(None).is_empty());
    }
}
