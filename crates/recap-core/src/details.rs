//! Match details: character, map, mode, K/D/A, result, score, plus any custom field.
//!
//! Every value remembers where it came from. Sources are ranked; a value is only replaced by one
//! from an equal or better source, and anything the player typed is never replaced.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::events::{kind, EventSource, GameEvent};

/// Hidden event kind carrying a details snapshot from a live API (meta = {field: value}).
/// Kinds starting with `_` are not shown on the timeline.
pub const SNAPSHOT_KIND: &str = "_details";

/// Well-known detail fields. Readers and APIs may use any other id too.
pub mod field {
    pub const CHARACTER: &str = "character";
    pub const MAP: &str = "map";
    pub const MODE: &str = "mode";
    pub const KILLS: &str = "kills";
    pub const DEATHS: &str = "deaths";
    pub const ASSISTS: &str = "assists";
    pub const RESULT: &str = "result";
    pub const SCORE: &str = "score";
}

/// Built-in fields in display order: (id, label).
pub const BUILTIN_FIELDS: [(&str, &str); 8] = [
    (field::CHARACTER, "Character"),
    (field::MAP, "Map"),
    (field::MODE, "Mode"),
    (field::KILLS, "Kills"),
    (field::DEATHS, "Deaths"),
    (field::ASSISTS, "Assists"),
    (field::RESULT, "Result"),
    (field::SCORE, "Score"),
];

/// Where a detail value came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DetailSource {
    /// Typed by the player.
    Manual,
    /// A local game API (League Live Client, CS2 GSI, the Riot client on localhost).
    Api,
    /// Read from the screen by a reader (text recognition or icon matching).
    Screen,
    /// Counted from detected events (rough).
    Events,
}

impl DetailSource {
    fn rank(self) -> u8 {
        match self {
            DetailSource::Manual => 0,
            DetailSource::Api => 1,
            DetailSource::Screen => 2,
            DetailSource::Events => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailValue {
    pub value: String,
    pub source: DetailSource,
    /// Reader confidence (0..1) for screen values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl DetailValue {
    pub fn new(value: impl Into<String>, source: DetailSource) -> Self {
        Self {
            value: value.into(),
            source,
            confidence: None,
        }
    }

    pub fn with_confidence(mut self, c: f32) -> Self {
        self.confidence = Some(c);
        self
    }
}

/// field id -> value.
pub type MatchDetails = BTreeMap<String, DetailValue>;

/// Put `v` into `details[field]` unless a better-ranked value is already there. Empty values are
/// ignored except from the player, where an empty value clears the field.
pub fn offer(details: &mut MatchDetails, field: &str, v: DetailValue) {
    let value = v.value.trim().to_string();
    if v.source == DetailSource::Manual {
        if value.is_empty() {
            details.remove(field);
        } else {
            details.insert(field.to_string(), DetailValue { value, ..v });
        }
        return;
    }
    if value.is_empty() {
        return;
    }
    match details.get(field) {
        Some(cur) if cur.source.rank() < v.source.rank() => {}
        Some(cur) if cur.source == v.source && cur.confidence.unwrap_or(1.0) > v.confidence.unwrap_or(1.0) => {}
        _ => {
            details.insert(field.to_string(), DetailValue { value, ..v });
        }
    }
}

/// Merge `incoming` into `details` value by value (see [`offer`]).
pub fn merge(details: &mut MatchDetails, incoming: &MatchDetails) {
    for (k, v) in incoming {
        offer(details, k, v.clone());
    }
}

/// Keep only what the player typed (used before recomputing everything else on a rescan).
pub fn retain_manual(details: &mut MatchDetails) {
    details.retain(|_, v| v.source == DetailSource::Manual);
}

/// Rough K/D/A counted from detected events.
pub fn from_events(events: &[GameEvent]) -> MatchDetails {
    let count = |k: &str| events.iter().filter(|e| e.kind == k).count();
    let mut d = MatchDetails::new();
    for (f, k) in [
        (field::KILLS, kind::KILL),
        (field::DEATHS, kind::DEATH),
        (field::ASSISTS, kind::ASSIST),
    ] {
        let n = count(k);
        if n > 0 {
            d.insert(f.into(), DetailValue::new(n.to_string(), DetailSource::Events));
        }
    }
    let wins = count(kind::ROUND_WIN);
    let losses = count(kind::ROUND_LOSS);
    if wins + losses > 0 {
        d.insert(
            field::SCORE.into(),
            DetailValue::new(format!("{wins}-{losses}"), DetailSource::Events),
        );
    }
    d
}

/// Normalise a result word from a game or the screen: "VICTORY", "Win", "DEFEAT", "lose", "draw".
pub fn normalize_result(s: &str) -> Option<&'static str> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    if ["victory", "win", "won", "winner", "1st", "#1"].iter().any(|w| s.contains(w)) {
        Some("win")
    } else if ["defeat", "loss", "lose", "lost"].iter().any(|w| s.contains(w)) {
        Some("loss")
    } else if ["draw", "tie"].iter().any(|w| s.contains(w)) {
        Some("draw")
    } else {
        None
    }
}

/// Readers may fill `team_score` and `enemy_score` separately: turn them into `score` ("13-11")
/// and, if nothing better says otherwise, the result.
pub fn combine_team_scores(d: &mut MatchDetails) {
    let (Some(a), Some(b)) = (d.remove("team_score"), d.remove("enemy_score")) else {
        return;
    };
    let (Ok(x), Ok(y)) = (a.value.parse::<i64>(), b.value.parse::<i64>()) else {
        return;
    };
    let conf = match (a.confidence, b.confidence) {
        (Some(p), Some(q)) => Some(p.min(q)),
        _ => None,
    };
    let mk = |v: String| DetailValue {
        value: v,
        source: a.source,
        confidence: conf,
    };
    offer(d, field::SCORE, mk(format!("{x}-{y}")));
    offer(d, field::RESULT, mk(result_from_score(x, y).to_string()));
}

/// When there is a score ("13-11") but no result, derive the result from it (same source).
pub fn result_from_score_detail(d: &mut MatchDetails) {
    if d.contains_key(field::RESULT) {
        return;
    }
    let Some(sc) = d.get(field::SCORE).cloned() else { return };
    let mut parts = sc.value.split(['-', ':', '–']).map(|p| p.trim().parse::<i64>());
    if let (Some(Ok(a)), Some(Ok(b)), None) = (parts.next(), parts.next(), parts.next()) {
        d.insert(
            field::RESULT.into(),
            DetailValue {
                value: result_from_score(a, b).into(),
                ..sc
            },
        );
    }
}

/// Result from a "mine-theirs" score, e.g. 13-9 -> win.
pub fn result_from_score(mine: i64, theirs: i64) -> &'static str {
    match mine.cmp(&theirs) {
        std::cmp::Ordering::Greater => "win",
        std::cmp::Ordering::Less => "loss",
        std::cmp::Ordering::Equal => "draw",
    }
}

/// Tracks API details and emits a snapshot event only when something changed.
#[derive(Debug, Default)]
pub struct SnapshotEmitter {
    last: BTreeMap<String, String>,
}

impl SnapshotEmitter {
    pub fn update(&mut self, values: BTreeMap<String, String>, t_ms: u64, source_name: &str) -> Option<GameEvent> {
        let values: BTreeMap<String, String> = values.into_iter().filter(|(_, v)| !v.trim().is_empty()).collect();
        if values.is_empty() || values == self.last {
            return None;
        }
        self.last = values.clone();
        Some(
            GameEvent::new(SNAPSHOT_KIND, t_ms, EventSource::Api { name: source_name.into() })
                .with_meta(serde_json::to_value(values).unwrap_or_default()),
        )
    }
}

/// The latest API snapshot among `events` (by time), as details.
pub fn latest_snapshot(events: &[GameEvent]) -> MatchDetails {
    let mut out = MatchDetails::new();
    if let Some(ev) = events.iter().filter(|e| e.kind == SNAPSHOT_KIND).max_by_key(|e| e.t_ms) {
        if let Some(obj) = ev.meta.as_object() {
            for (k, v) in obj {
                let value = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                out.insert(k.clone(), DetailValue::new(value, DetailSource::Api));
            }
        }
    }
    out
}

/// "de_mirage" -> "Mirage", "competitive" -> "Competitive".
pub fn pretty_name(raw: &str) -> String {
    let s = raw.trim();
    let s = ["de_", "cs_", "ar_", "dz_", "gd_"]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    s.split(['_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSource;

    #[test]
    fn ranking_and_manual_edits() {
        let mut d = MatchDetails::new();
        offer(&mut d, field::KILLS, DetailValue::new("3", DetailSource::Events));
        offer(
            &mut d,
            field::KILLS,
            DetailValue::new("4", DetailSource::Screen).with_confidence(0.6),
        );
        assert_eq!(d[field::KILLS].value, "4");
        // A lower-confidence screen read does not replace a better one.
        offer(
            &mut d,
            field::KILLS,
            DetailValue::new("9", DetailSource::Screen).with_confidence(0.4),
        );
        assert_eq!(d[field::KILLS].value, "4");
        offer(&mut d, field::KILLS, DetailValue::new("5", DetailSource::Api));
        offer(
            &mut d,
            field::KILLS,
            DetailValue::new("4", DetailSource::Screen).with_confidence(0.99),
        );
        assert_eq!(d[field::KILLS].value, "5");
        offer(&mut d, field::KILLS, DetailValue::new(" 6 ", DetailSource::Manual));
        offer(&mut d, field::KILLS, DetailValue::new("7", DetailSource::Api));
        assert_eq!(d[field::KILLS], DetailValue::new("6", DetailSource::Manual));
        offer(&mut d, field::MAP, DetailValue::new("Ascent", DetailSource::Screen));
        retain_manual(&mut d);
        assert_eq!(d.len(), 1);
        offer(&mut d, field::KILLS, DetailValue::new("", DetailSource::Manual));
        assert!(d.is_empty());
    }

    #[test]
    fn snapshots() {
        let mut em = SnapshotEmitter::default();
        let vals = |k: &str| BTreeMap::from([("map".to_string(), "Mirage".to_string()), ("kills".to_string(), k.to_string())]);
        let a = em.update(vals("1"), 1000, "x").unwrap();
        assert!(em.update(vals("1"), 2000, "x").is_none());
        let b = em.update(vals("2"), 3000, "x").unwrap();
        let d = latest_snapshot(&[b, a]);
        assert_eq!(d["kills"], DetailValue::new("2", DetailSource::Api));
        assert_eq!(pretty_name("de_mirage"), "Mirage");
        assert_eq!(pretty_name("spike_rush"), "Spike Rush");
    }

    #[test]
    fn events_and_results() {
        let v = |k: &str| GameEvent::new(k, 0, EventSource::Vision { detector: "x".into() });
        let d = from_events(&[v("kill"), v("kill"), v("death"), v("round_win"), v("round_loss"), v("round_win")]);
        assert_eq!(d[field::KILLS].value, "2");
        assert_eq!(d[field::DEATHS].value, "1");
        assert!(!d.contains_key(field::ASSISTS));
        assert_eq!(d[field::SCORE].value, "2-1");
        assert_eq!(normalize_result("VICTORY"), Some("win"));
        assert_eq!(normalize_result(" Defeat "), Some("loss"));
        assert_eq!(normalize_result("DRAW"), Some("draw"));
        assert_eq!(normalize_result("ascent"), None);
        assert_eq!(result_from_score(13, 9), "win");
        let mut d = MatchDetails::new();
        d.insert("team_score".into(), DetailValue::new("11", DetailSource::Screen));
        d.insert("enemy_score".into(), DetailValue::new("13", DetailSource::Screen));
        combine_team_scores(&mut d);
        assert_eq!((d[field::SCORE].value.as_str(), d[field::RESULT].value.as_str()), ("11-13", "loss"));
        assert!(!d.contains_key("team_score"));
        let mut d = MatchDetails::new();
        d.insert(field::SCORE.into(), DetailValue::new("13-7", DetailSource::Api));
        result_from_score_detail(&mut d);
        assert_eq!(d[field::RESULT], DetailValue::new("win", DetailSource::Api));
        let s = serde_json::to_string(&DetailValue::new("Jett", DetailSource::Screen).with_confidence(0.9)).unwrap();
        assert_eq!(s, r#"{"value":"Jett","source":"screen","confidence":0.9}"#);
    }
}
