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
    pub const ROUND_START: &str = "round_start";
    pub const ROUND_WIN: &str = "round_win";
    pub const ROUND_LOSS: &str = "round_loss";
    pub const MVP: &str = "mvp";
    pub const BOMB_PLANT: &str = "bomb_plant";
    pub const BOMB_DEFUSE: &str = "bomb_defuse";
    pub const BOOKMARK: &str = "bookmark";
    pub const MATCH_START: &str = "match_start";
    pub const MATCH_END: &str = "match_end";
}

/// The 15 icons an event type can use (the UI draws them).
pub const ICONS: [&str; 15] = [
    "crosshair",
    "skull",
    "handshake",
    "flame",
    "crown",
    "gem",
    "flag",
    "trophy",
    "circle-x",
    "star",
    "bomb",
    "shield",
    "bookmark",
    "zap",
    "heart",
];

/// Kinds that mark round boundaries/results.
pub fn is_round_kind(k: &str) -> bool {
    matches!(k, kind::ROUND_START | kind::ROUND_WIN | kind::ROUND_LOSS)
}

/// A named event type: what the timeline calls an event kind, and how it looks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTypeDef {
    /// The event `kind`, e.g. `kill` or `ult_used`.
    pub id: String,
    pub label: String,
    /// One of [`ICONS`].
    pub icon: String,
    /// Hex colour.
    pub color: String,
    /// Built-in types are not stored in game files.
    #[serde(default, skip_deserializing)]
    pub builtin: bool,
}

impl EventTypeDef {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            anyhow::bail!("event type id {:?} must use lowercase letters, digits and _", self.id);
        }
        if self.label.trim().is_empty() {
            anyhow::bail!("event type needs a name");
        }
        if !ICONS.contains(&self.icon.as_str()) {
            anyhow::bail!("unknown icon {:?}", self.icon);
        }
        let hex = self.color.strip_prefix('#').unwrap_or("");
        if !(hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())) {
            anyhow::bail!("colour must be a hex value like #6fd6a8");
        }
        Ok(())
    }
}

/// Built-in event types, in display order.
pub fn builtin_event_types() -> Vec<EventTypeDef> {
    [
        (kind::KILL, "Kill", "crosshair", "#7cc3a3"),
        (kind::DEATH, "Death", "skull", "#d68b80"),
        (kind::ASSIST, "Assist", "handshake", "#8fa7d6"),
        (kind::MULTIKILL, "Multikill", "flame", "#ad9bd3"),
        (kind::ACE, "Ace", "crown", "#cfa1bd"),
        (kind::OBJECTIVE, "Objective", "gem", "#7cb6be"),
        (kind::ROUND_START, "Round start", "flag", "#7d818a"),
        (kind::ROUND_WIN, "Round won", "trophy", "#9dbb7c"),
        (kind::ROUND_LOSS, "Round lost", "circle-x", "#a08a90"),
        (kind::MVP, "MVP", "star", "#c9b98a"),
        (kind::BOMB_PLANT, "Bomb planted", "bomb", "#d39b74"),
        (kind::BOMB_DEFUSE, "Bomb defused", "shield", "#80acc9"),
        (kind::BOOKMARK, "Bookmark", "bookmark", "#f2a93b"),
        (kind::MATCH_START, "Match start", "flag", "#7d818a"),
        (kind::MATCH_END, "Match end", "flag", "#7d818a"),
    ]
    .into_iter()
    .map(|(id, label, icon, color)| EventTypeDef {
        id: id.into(),
        label: label.into(),
        icon: icon.into(),
        color: color.into(),
        builtin: true,
    })
    .collect()
}

/// Turn a free-text name into an event type id: "Ult used!" -> "ult_used".
pub fn slugify(label: &str) -> String {
    let mut out = String::new();
    for c in label.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').chars().take(40).collect()
}

/// Colours given to custom types when the user does not pick one.
pub const AUTO_COLORS: [&str; 8] = [
    "#7cc3a3", "#d68b80", "#8fa7d6", "#ad9bd3", "#cfa1bd", "#7cb6be", "#c9b98a", "#d39b74",
];

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

/// Drop screen detections that the game's round rules make impossible. Only vision events are
/// touched (API and manual events are trusted):
///
/// * deaths during the setup phase after a round starts (the death screen of the last round);
/// * with one life per round, a second death in the same round, and kills/assists after your
///   death (the spectated teammate's banners).
///
/// A round boundary is a `round_start`, `round_win` or `round_loss` from any source.
pub fn apply_round_rules(events: Vec<GameEvent>, rules: &crate::games::RoundRules) -> Vec<GameEvent> {
    let mut events = events;
    events.sort_by_key(|e| e.t_ms);
    let vision = |e: &GameEvent| matches!(e.source, EventSource::Vision { .. });
    let mut round_start: Option<u64> = None;
    let mut dead_at: Option<u64> = None;
    let mut out = Vec::with_capacity(events.len());
    for e in events {
        match e.kind.as_str() {
            kind::ROUND_START => {
                round_start = Some(e.t_ms);
                dead_at = None;
            }
            kind::ROUND_WIN | kind::ROUND_LOSS => {
                round_start = None;
                dead_at = None;
            }
            _ => {}
        }
        let dead = dead_at.is_some_and(|d| rules.max_after_death_ms == 0 || e.t_ms.saturating_sub(d) <= rules.max_after_death_ms);
        if vision(&e) {
            let in_setup = rules.setup_ms > 0 && round_start.is_some_and(|s| e.t_ms.saturating_sub(s) < rules.setup_ms);
            match e.kind.as_str() {
                kind::DEATH if in_setup || (rules.one_life && dead) => continue,
                kind::KILL | kind::ASSIST | kind::MULTIKILL | kind::ACE if rules.one_life && dead => continue,
                _ => {}
            }
        }
        if e.kind == kind::DEATH && rules.one_life {
            dead_at = Some(e.t_ms);
        }
        out.push(e);
    }
    out
}

/// Kills this close together (and in the same round) count as one multikill.
pub const MULTIKILL_GAP_MS: u64 = 10_000;

/// Multikills (and aces) from the kills on a timeline, for games whose API does not report them:
/// a chain of 2+ kills, each within `gap_ms` of the previous one and with no round boundary in
/// between, becomes one `multikill` at its first kill (`meta.count`, `meta.endMs`). In round-based
/// games, 5 kills in one round add an `ace`. Nothing is derived when the timeline already has
/// multikill/ace events from an exact source.
pub fn derive_multikills(events: &[GameEvent], gap_ms: u64) -> Vec<GameEvent> {
    let has = |k: &str| {
        events
            .iter()
            .any(|e| e.kind == k && !matches!(e.source, EventSource::PostMatch { .. }))
    };
    let mut sorted: Vec<&GameEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.t_ms);
    let src = || EventSource::PostMatch {
        processor: "multikill".into(),
    };
    let mut out = Vec::new();
    let mut chain: Vec<u64> = Vec::new();
    let mut round_kills: Vec<u64> = Vec::new();
    let mut has_rounds = false;
    let flush_chain = |chain: &mut Vec<u64>, out: &mut Vec<GameEvent>| {
        if chain.len() >= 2 && !has(kind::MULTIKILL) {
            out.push(
                GameEvent::new(kind::MULTIKILL, chain[0], src())
                    .with_meta(serde_json::json!({ "count": chain.len(), "endMs": chain[chain.len() - 1] })),
            );
        }
        chain.clear();
    };
    let flush_round = |kills: &mut Vec<u64>, out: &mut Vec<GameEvent>, has_rounds: bool| {
        if has_rounds && kills.len() >= 5 && !has(kind::ACE) {
            out.push(
                GameEvent::new(kind::ACE, kills[0], src())
                    .with_meta(serde_json::json!({ "count": kills.len(), "endMs": kills[kills.len() - 1] })),
            );
        }
        kills.clear();
    };
    for e in sorted {
        match e.kind.as_str() {
            kind::ROUND_START | kind::ROUND_WIN | kind::ROUND_LOSS => {
                has_rounds = true;
                flush_chain(&mut chain, &mut out);
                flush_round(&mut round_kills, &mut out, has_rounds);
            }
            kind::KILL => {
                if chain.last().is_some_and(|&l| e.t_ms.saturating_sub(l) > gap_ms) {
                    flush_chain(&mut chain, &mut out);
                }
                chain.push(e.t_ms);
                round_kills.push(e.t_ms);
            }
            _ => {}
        }
    }
    flush_chain(&mut chain, &mut out);
    flush_round(&mut round_kills, &mut out, has_rounds);
    out
}

/// Drop vision events matched by an exclusion rule (see [`crate::games::Exclusion`]).
pub fn apply_exclusions(events: Vec<GameEvent>, rules: &[crate::games::Exclusion]) -> Vec<GameEvent> {
    if rules.is_empty() {
        return events;
    }
    let keep: Vec<bool> = events
        .iter()
        .map(|e| {
            !matches!(e.source, EventSource::Vision { .. })
                || !rules.iter().any(|r| {
                    r.drop == e.kind
                        && events
                            .iter()
                            .any(|o| o.kind == r.near && e.t_ms + r.before_ms >= o.t_ms && e.t_ms <= o.t_ms + r.after_ms)
                })
        })
        .collect();
    events.into_iter().zip(keep).filter_map(|(e, k)| k.then_some(e)).collect()
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
    fn multikills_from_kill_chains() {
        let v = |k: &str, t: u64| GameEvent::new(k, t, EventSource::Vision { detector: "d".into() });
        let evs = vec![
            v(kind::ROUND_START, 0),
            v(kind::KILL, 10_000),
            v(kind::KILL, 14_000),
            v(kind::KILL, 20_000), // triple
            v(kind::KILL, 45_000), // alone
            v(kind::KILL, 50_000),
            v(kind::ROUND_WIN, 52_000),
            v(kind::KILL, 53_000), // next round: not chained with 50 s
        ];
        let d = derive_multikills(&evs, MULTIKILL_GAP_MS);
        let got: Vec<(String, u64, u64)> = d
            .iter()
            .map(|e| (e.kind.clone(), e.t_ms, e.meta["count"].as_u64().unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                (kind::MULTIKILL.into(), 10_000, 3),
                (kind::MULTIKILL.into(), 45_000, 2),
                (kind::ACE.into(), 10_000, 5)
            ]
        );
        // An API that reports multikills itself: nothing derived.
        let mut api = evs.clone();
        api.push(GameEvent::new(kind::MULTIKILL, 15_000, EventSource::Api { name: "x".into() }));
        assert!(derive_multikills(&api, MULTIKILL_GAP_MS).iter().all(|e| e.kind != kind::MULTIKILL));
    }

    #[test]
    fn exclusions_drop_events_near_another_kind() {
        let v = |k: &str, t: u64| GameEvent::new(k, t, EventSource::Vision { detector: "d".into() });
        let rules = [crate::games::Exclusion {
            drop: kind::KILL.into(),
            near: kind::DEATH.into(),
            before_ms: 1000,
            after_ms: 8000,
        }];
        let evs = vec![
            v(kind::KILL, 1000),
            v(kind::KILL, 9_500), // the feed entry of your own death
            v(kind::DEATH, 10_000),
            v(kind::KILL, 17_500), // the death replay shows it again
            v(kind::KILL, 19_000),
        ];
        let kept: Vec<u64> = apply_exclusions(evs, &rules).into_iter().map(|e| e.t_ms).collect();
        assert_eq!(kept, vec![1000, 10_000, 19_000]);
    }

    #[test]
    fn round_rules_drop_impossible_detections() {
        let v = |k: &str, t: u64| GameEvent::new(k, t, EventSource::Vision { detector: "d".into() });
        let rules = crate::games::RoundRules {
            one_life: true,
            setup_ms: 30_000,
            max_after_death_ms: 145_000,
        };
        let evs = vec![
            v(kind::ROUND_START, 0),
            v(kind::DEATH, 10_000), // last round's death screen during the buy phase
            v(kind::KILL, 40_000),
            v(kind::DEATH, 50_000),
            v(kind::DEATH, 58_000), // same death, shown again
            v(kind::KILL, 60_000),  // spectating a teammate
            v(kind::ROUND_START, 100_000),
            v(kind::KILL, 140_000),
            v(kind::DEATH, 150_000),
            // The next round start was missed: detections come back after the longest round.
            v(kind::KILL, 400_000),
        ];
        let kept: Vec<(String, u64)> = apply_round_rules(evs, &rules).into_iter().map(|e| (e.kind, e.t_ms)).collect();
        let want = [
            (kind::ROUND_START, 0),
            (kind::KILL, 40_000),
            (kind::DEATH, 50_000),
            (kind::ROUND_START, 100_000),
            (kind::KILL, 140_000),
            (kind::DEATH, 150_000),
            (kind::KILL, 400_000),
        ];
        assert_eq!(kept, want.iter().map(|(k, t)| (k.to_string(), *t)).collect::<Vec<_>>());
    }

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
    fn event_types() {
        let b = builtin_event_types();
        assert_eq!(b.len(), 15);
        assert!(b.iter().all(|t| t.validate().is_ok() && t.builtin));
        assert!(b.iter().all(|t| ICONS.contains(&t.icon.as_str())));
        assert_eq!(slugify("  Ult used!! "), "ult_used");
        assert_eq!(slugify("Clutch 1v3"), "clutch_1v3");
        let bad = EventTypeDef {
            id: "x".into(),
            label: "X".into(),
            icon: "sword".into(),
            color: "#fff".into(),
            builtin: false,
        };
        assert!(bad.validate().is_err());
        assert!(is_round_kind("round_start") && !is_round_kind("kill"));
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
