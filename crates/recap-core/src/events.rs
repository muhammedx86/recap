//! The single event type every source produces.

use serde::{Deserialize, Serialize};

/// Well-known event kinds. Detector files may use any string; these are the
/// ones the UI knows how to label and colour.
pub mod kind {
    pub const KILL: &str = "kill";
    pub const DEATH: &str = "death";
    pub const ASSIST: &str = "assist";
    pub const MULTIKILL: &str = "multikill";
    pub const ACE: &str = "ace";
    pub const OBJECTIVE: &str = "objective";
    pub const ROUND_WIN: &str = "round_win";
    pub const ROUND_LOSS: &str = "round_loss";
    pub const MVP: &str = "mvp";
    pub const BOMB_PLANT: &str = "bomb_plant";
    pub const BOMB_DEFUSE: &str = "bomb_defuse";
    pub const BOOKMARK: &str = "bookmark";
    pub const MATCH_START: &str = "match_start";
    pub const MATCH_END: &str = "match_end";
}

/// Where an event came from. Purely informational: nothing downstream branches on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EventSource {
    /// A vision detector defined in a data file (`detector` is its id).
    Vision { detector: String },
    /// An official local game API (e.g. `league-live-client`, `cs2-gsi`).
    Api { name: String },
    /// A post-match processor reading files the game wrote.
    PostMatch { processor: String },
    /// The bookmark hotkey or a manual marker from the player.
    Manual,
}

impl EventSource {
    pub fn label(&self) -> String {
        match self {
            EventSource::Vision { detector } => format!("vision:{detector}"),
            EventSource::Api { name } => format!("api:{name}"),
            EventSource::PostMatch { processor } => format!("postmatch:{processor}"),
            EventSource::Manual => "manual".into(),
        }
    }
}

/// One moment on a recording's timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEvent {
    /// Row id when read from the library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Event kind, e.g. `kill`. See [`kind`].
    pub kind: String,
    /// Milliseconds from the start of the recording.
    pub t_ms: u64,
    pub source: EventSource,
    /// Match score for vision events (0..1), None for exact sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// Free-form extra data (victim name, multikill count, round number...).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub meta: serde_json::Value,
}

impl GameEvent {
    pub fn new(kind: impl Into<String>, t_ms: u64, source: EventSource) -> Self {
        Self {
            id: None,
            kind: kind.into(),
            t_ms,
            source,
            confidence: None,
            meta: serde_json::Value::Null,
        }
    }

    pub fn with_confidence(mut self, c: f32) -> Self {
        self.confidence = Some(c);
        self
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = meta;
        self
    }
}

/// Merge events from several sources, dropping near-duplicates of the same kind
/// (e.g. a vision kill and an API kill for the same moment). When two events of
/// the same kind fall within `window_ms`, the one with an exact source (no
/// confidence) wins.
pub fn merge_events(mut events: Vec<GameEvent>, window_ms: u64) -> Vec<GameEvent> {
    events.sort_by_key(|e| e.t_ms);
    let mut out: Vec<GameEvent> = Vec::with_capacity(events.len());
    for ev in events {
        if let Some(prev) = out
            .iter_mut()
            .rev()
            .take_while(|p| ev.t_ms.saturating_sub(p.t_ms) <= window_ms)
            .find(|p| p.kind == ev.kind && p.source != ev.source && !is_manual(p) && !is_manual(&ev))
        {
            // Prefer exact sources over vision guesses.
            if prev.confidence.is_some() && ev.confidence.is_none() {
                *prev = ev;
            }
            continue;
        }
        out.push(ev);
    }
    out
}

fn is_manual(e: &GameEvent) -> bool {
    matches!(e.source, EventSource::Manual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_prefers_exact_source() {
        let vision = GameEvent::new(kind::KILL, 10_000, EventSource::Vision { detector: "k".into() }).with_confidence(0.9);
        let api = GameEvent::new(
            kind::KILL,
            10_400,
            EventSource::Api {
                name: "league-live-client".into(),
            },
        );
        let other = GameEvent::new(kind::KILL, 30_000, EventSource::Vision { detector: "k".into() });
        let merged = merge_events(vec![vision, api.clone(), other], 1500);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], api);
    }

    #[test]
    fn merge_keeps_same_source_repeats() {
        let a = GameEvent::new(kind::KILL, 1000, EventSource::Api { name: "x".into() });
        let b = GameEvent::new(kind::KILL, 1200, EventSource::Api { name: "x".into() });
        assert_eq!(merge_events(vec![a, b], 1500).len(), 2);
    }

    #[test]
    fn serde_roundtrip_is_camel_case() {
        let e = GameEvent::new(kind::DEATH, 5, EventSource::Vision { detector: "d".into() }).with_confidence(0.5);
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"tMs\":5"));
        assert!(s.contains("\"type\":\"vision\""));
        let back: GameEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
    }
}
