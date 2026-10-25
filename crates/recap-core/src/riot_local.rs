//! VALORANT match details from the Riot Client's local API (localhost only).
//!
//! While the Riot Client runs it writes a `lockfile`
//! (`%LOCALAPPDATA%\Riot Games\Riot Client\Config\lockfile`, `name:pid:port:password:protocol`)
//! and serves an HTTPS API on `127.0.0.1:<port>` (self-signed certificate, basic auth
//! `riot:<password>`). Recap only reads two local endpoints:
//!
//! * `GET /chat/v1/session` → the local player's `puuid`
//! * `GET /chat/v4/presences` → presences; ours carries a base64 JSON blob with the session state,
//!   map, queue and the live team score.
//!
//! No Riot web service is called. The agent is not in the presence, so it comes from a screen
//! reader (Detector Studio).

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::Value;

use serde_json::json;

use crate::{
    details::{field, pretty_name, result_from_score, SnapshotEmitter},
    events::{kind, EventSource, GameEvent},
};

pub const SOURCE_NAME: &str = "riot-client";

#[derive(Debug, Clone, PartialEq)]
pub struct Lockfile {
    pub port: u16,
    pub password: String,
    pub protocol: String,
}

impl Lockfile {
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.trim().split(':').collect();
        if parts.len() < 5 {
            return None;
        }
        Some(Self {
            port: parts[2].parse().ok()?,
            password: parts[3].to_string(),
            protocol: parts[4].to_string(),
        })
    }

    pub fn base_url(&self) -> String {
        format!("{}://127.0.0.1:{}", self.protocol, self.port)
    }

    pub fn auth_header(&self) -> String {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("riot:{}", self.password));
        format!("Basic {token}")
    }
}

/// Default lockfile location on Windows.
pub fn lockfile_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join(r"Riot Games\Riot Client\Config\lockfile"))
}

/// Internal map path ("/Game/Maps/Duality/Duality") -> display name.
pub fn map_display_name(path: &str) -> String {
    let code = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path);
    let known = match code.to_ascii_lowercase().as_str() {
        "ascent" => "Ascent",
        "duality" => "Bind",
        "bonsai" => "Split",
        "triad" => "Haven",
        "port" => "Icebox",
        "foxtrot" => "Breeze",
        "canyon" => "Fracture",
        "pitt" => "Pearl",
        "jam" => "Lotus",
        "juliett" => "Sunset",
        "infinity" => "Abyss",
        "plummet" => "Summit",
        "rook" => "Corrode",
        "abilitydraft" => "Gauntlet",
        "range" | "rangev2" | "poveglia" => "The Range",
        "npev2" => "Basic Training",
        "hurm_hightide" => "Glitch",
        "hurm_alley" => "District",
        "hurm_yard" => "Piazza",
        "hurm_bowl" => "Kasbah",
        "hurm_helix" => "Drift",
        _ => "",
    };
    if known.is_empty() {
        pretty_name(code)
    } else {
        known.to_string()
    }
}

/// Queue id -> mode name.
pub fn mode_display_name(queue: &str, custom: bool) -> String {
    if custom {
        return "Custom".into();
    }
    match queue.to_ascii_lowercase().as_str() {
        "competitive" => "Competitive".into(),
        "unrated" => "Unrated".into(),
        "swiftplay" => "Swiftplay".into(),
        "spikerush" => "Spike Rush".into(),
        "deathmatch" => "Deathmatch".into(),
        "ggteam" => "Escalation".into(),
        "onefa" => "Replication".into(),
        "hurm" => "Team Deathmatch".into(),
        "premier" => "Premier".into(),
        "newmap" => "New Map".into(),
        "" => String::new(),
        other => pretty_name(other),
    }
}

/// Find the first value for `key` anywhere in a JSON tree (the presence layout has moved
/// between client versions: flat in older ones, nested under `matchPresenceData` etc. now).
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|c| find(c, key))),
        Value::Array(a) => a.iter().find_map(|c| find(c, key)),
        _ => None,
    }
}

/// Decode our presence's private blob.
pub fn decode_private(presences: &Value, puuid: &str) -> Option<Value> {
    let p = presences["presences"]
        .as_array()?
        .iter()
        .find(|p| p["puuid"].as_str() == Some(puuid) && p["product"].as_str().is_none_or(|x| x == "valorant"))?;
    let raw = p["private"].as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Tracks the presence across polls and emits details snapshots.
#[derive(Debug, Default)]
pub struct RiotTracker {
    details: SnapshotEmitter,
    in_game: bool,
    map: Option<String>,
    mode: Option<String>,
    score: Option<(i64, i64)>,
    result: Option<String>,
    rank: Option<String>,
    player: Option<String>,
    character: Option<String>,
}

impl RiotTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Your agent, from the game's log (see [`crate::valorant_log`]).
    pub fn set_character(&mut self, name: Option<String>) {
        if name.is_some() {
            self.character = name;
        }
    }

    /// The player's Riot ID ("name#tag"), from the client's chat session.
    pub fn set_player(&mut self, riot_id: Option<String>) {
        if riot_id.is_some() {
            self.player = riot_id;
        }
    }

    /// Feed one decoded private presence at `rec_elapsed_ms`.
    /// Feed one decoded private presence at `rec_elapsed_ms`. Returns round won/lost events when
    /// the score changes, and a details snapshot when the details changed.
    pub fn ingest(&mut self, private: &Value, rec_elapsed_ms: u64) -> Vec<GameEvent> {
        let mut out = Vec::new();
        let state = find(private, "sessionLoopState").and_then(Value::as_str).unwrap_or("");
        let now_in_game = state.eq_ignore_ascii_case("INGAME");
        if now_in_game {
            if !self.in_game {
                // A new match: forget the previous result and score.
                self.result = None;
                self.score = None;
            }
            if let Some(m) = find(private, "matchMap").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                self.map = Some(map_display_name(m));
            }
            let custom = find(private, "provisioningFlow").and_then(Value::as_str) == Some("CustomGame");
            let queue = find(private, "queueId").and_then(Value::as_str).unwrap_or("");
            let mode = mode_display_name(queue, custom);
            if !mode.is_empty() {
                self.mode = Some(mode);
            }
            let num = |k: &str| find(private, k).and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
            if let Some(name) = num("competitiveTier").and_then(rank_name) {
                self.rank = Some(name.to_string());
            }
            if let (Some(a), Some(e)) = (num("partyOwnerMatchScoreAllyTeam"), num("partyOwnerMatchScoreEnemyTeam")) {
                // A score change is a round result (the first score seen is only a baseline).
                if let Some((pa, pe)) = self.score {
                    let round = |n: i64| {
                        GameEvent::new(kind::ROUND_WIN, rec_elapsed_ms, EventSource::Api { name: SOURCE_NAME.into() })
                            .with_meta(json!({ "round": n }))
                    };
                    if a > pa {
                        out.push(round(a + e));
                    }
                    if e > pe {
                        let mut ev = round(a + e);
                        ev.kind = kind::ROUND_LOSS.into();
                        out.push(ev);
                    }
                }
                self.score = Some((a, e));
            }
        } else if self.in_game {
            // Left the match: the last score decides the result.
            if let Some((a, e)) = self.score {
                self.result = Some(result_from_score(a, e).to_string());
            }
        }
        self.in_game = now_in_game;

        let mut d = BTreeMap::new();
        if let Some(m) = &self.map {
            d.insert(field::MAP.to_string(), m.clone());
        }
        if let Some(m) = &self.mode {
            d.insert(field::MODE.to_string(), m.clone());
        }
        if let Some((a, e)) = self.score {
            d.insert(field::SCORE.to_string(), format!("{a}-{e}"));
        }
        if let Some(r) = &self.result {
            d.insert(field::RESULT.to_string(), r.clone());
        }
        if let Some(r) = &self.rank {
            d.insert(field::RANK.to_string(), r.clone());
        }
        if let Some(p) = &self.player {
            d.insert(field::PLAYER.to_string(), p.clone());
        }
        if let Some(c) = &self.character {
            d.insert(field::CHARACTER.to_string(), c.clone());
        }
        out.extend(self.details.update(d, rec_elapsed_ms, SOURCE_NAME));
        out
    }
}

/// VALORANT competitive tier number -> rank name (0 = unranked).
pub fn rank_name(tier: i64) -> Option<&'static str> {
    const NAMES: [&str; 25] = [
        "Iron 1",
        "Iron 2",
        "Iron 3",
        "Bronze 1",
        "Bronze 2",
        "Bronze 3",
        "Silver 1",
        "Silver 2",
        "Silver 3",
        "Gold 1",
        "Gold 2",
        "Gold 3",
        "Platinum 1",
        "Platinum 2",
        "Platinum 3",
        "Diamond 1",
        "Diamond 2",
        "Diamond 3",
        "Ascendant 1",
        "Ascendant 2",
        "Ascendant 3",
        "Immortal 1",
        "Immortal 2",
        "Immortal 3",
        "Radiant",
    ];
    match tier {
        0 => Some("Unranked"),
        3..=27 => Some(NAMES[(tier - 3) as usize]),
        _ => None,
    }
}

/// "name#tag" from the client's `/chat/v1/session` reply.
pub fn riot_id(session: &Value) -> Option<String> {
    let name = session["game_name"].as_str().filter(|s| !s.is_empty())?;
    match session["game_tag"].as_str().filter(|s| !s.is_empty()) {
        Some(tag) => Some(format!("{name}#{tag}")),
        None => Some(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::details::latest_snapshot;

    fn presence(puuid: &str, private: Value) -> Value {
        let b64 = base64::engine::general_purpose::STANDARD.encode(private.to_string());
        json!({ "presences": [
            { "puuid": "someone-else", "product": "valorant", "private": "e30=" },
            { "puuid": puuid, "product": "valorant", "private": b64 }
        ]})
    }

    #[test]
    fn lockfile_and_names() {
        let l = Lockfile::parse("Riot Client:1234:51234:pa55:https\n").unwrap();
        assert_eq!(l.base_url(), "https://127.0.0.1:51234");
        assert_eq!(l.auth_header(), "Basic cmlvdDpwYTU1");
        assert!(Lockfile::parse("garbage").is_none());
        assert_eq!(map_display_name("/Game/Maps/Duality/Duality"), "Bind");
        assert_eq!(map_display_name("/Game/Maps/Juliett/Juliett"), "Sunset");
        assert_eq!(map_display_name("/Game/Maps/Mystery/Mystery"), "Mystery");
        assert_eq!(mode_display_name("spikerush", false), "Spike Rush");
        assert_eq!(mode_display_name("competitive", true), "Custom");
        assert_eq!(rank_name(14), Some("Gold 3"));
        assert_eq!(rank_name(12), Some("Gold 1"));
        assert_eq!(rank_name(27), Some("Radiant"));
        assert_eq!(rank_name(1), None);
        assert_eq!(
            riot_id(&json!({"game_name": "11lumin", "game_tag": "1111"})).as_deref(),
            Some("11lumin#1111")
        );
    }

    #[test]
    fn tracks_old_and_new_presence_layouts() {
        let me = "me-puuid";
        // Older flat layout.
        let flat = json!({ "sessionLoopState": "INGAME", "matchMap": "/Game/Maps/Port/Port", "queueId": "competitive",
                           "partyOwnerMatchScoreAllyTeam": 5, "partyOwnerMatchScoreEnemyTeam": 3 });
        let p = decode_private(&presence(me, flat), me).unwrap();
        let mut t = RiotTracker::new();
        let mut evs: Vec<GameEvent> = t.ingest(&p, 1000);
        // Newer nested layout, later in the match.
        let nested = json!({ "matchPresenceData": { "sessionLoopState": "INGAME", "matchMap": "/Game/Maps/Port/Port", "queueId": "competitive" },
                             "partyPresenceData": { "partyOwnerMatchScoreAllyTeam": 13, "partyOwnerMatchScoreEnemyTeam": 11 } });
        evs.extend(t.ingest(&nested, 2000));
        // Back to the menus: result decided by the final score.
        evs.extend(t.ingest(&json!({ "matchPresenceData": { "sessionLoopState": "MENUS" } }), 3000));
        // 5-3 -> 13-11 between two polls: rounds won and lost were reported.
        let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).filter(|k| !k.starts_with('_')).collect();
        assert_eq!(kinds, vec!["round_win", "round_loss"]);
        let d = latest_snapshot(&evs);
        let v = |k: &str| d.get(k).map(|v| v.value.clone()).unwrap_or_default();
        assert_eq!(
            (v("map"), v("mode"), v("score"), v("result")),
            ("Icebox".into(), "Competitive".into(), "13-11".into(), "win".into())
        );
    }
}
