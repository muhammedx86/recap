//! Counter-Strike 2: Valve's Game State Integration (GSI).
//!
//! CS2 reads every `gamestate_integration_*.cfg` in its `cfg` folder at launch
//! and POSTs JSON game state to the configured local URI. The app installs
//! [`gsi_config`] as [`GSI_CFG_NAME`], runs a tiny HTTP listener on
//! 127.0.0.1, and feeds every payload to [`Cs2Tracker::ingest`].

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use crate::events::{kind, EventSource, GameEvent};

pub const GSI_CFG_NAME: &str = "gamestate_integration_recap.cfg";
pub const SOURCE_NAME: &str = "cs2-gsi";

/// Contents of the GSI config file (Valve KeyValues).
pub fn gsi_config(port: u16, token: &str) -> String {
    let token = token.replace('"', "");
    format!(
        r#""Recap Game State Integration"
{{
    "uri"       "http://127.0.0.1:{port}"
    "timeout"   "5.0"
    "buffer"    "0.1"
    "throttle"  "0.5"
    "heartbeat" "10.0"
    "auth"
    {{
        "token" "{token}"
    }}
    "data"
    {{
        "provider"            "1"
        "map"                 "1"
        "round"               "1"
        "player_id"           "1"
        "player_state"        "1"
        "player_match_stats"  "1"
        "map_round_wins"      "1"
    }}
}}
"#
    )
}

/// Quoted tokens of a Valve KeyValues file, with `\\` and `\"` unescaped.
fn vdf_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut tok = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        tok.push(n);
                    }
                }
                '"' => break,
                _ => tok.push(c),
            }
        }
        out.push(tok);
    }
    out
}

/// Steam library roots listed in `<steam_root>/steamapps/libraryfolders.vdf`, plus the root itself.
pub fn steam_libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    if let Ok(text) = fs::read_to_string(steam_root.join("steamapps").join("libraryfolders.vdf")) {
        let toks = vdf_tokens(&text);
        for w in toks.windows(2) {
            if w[0].eq_ignore_ascii_case("path") {
                libs.push(PathBuf::from(&w[1]));
            }
        }
    }
    let mut seen = HashSet::new();
    libs.retain(|p| {
        seen.insert(
            p.to_string_lossy()
                .to_lowercase()
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string(),
        )
    });
    libs
}

/// Existing CS2 `cfg` folders across all Steam libraries.
pub fn find_cs2_cfg_dirs(steam_root: &Path) -> Vec<PathBuf> {
    steam_libraries(steam_root)
        .into_iter()
        .map(|lib| {
            lib.join("steamapps")
                .join("common")
                .join("Counter-Strike Global Offensive")
                .join("game")
                .join("csgo")
                .join("cfg")
        })
        .filter(|p| p.is_dir())
        .collect()
}

/// Turns GSI payloads into events for the local player.
#[derive(Debug)]
pub struct Cs2Tracker {
    token: String,
    initialised: bool,
    my_team: Option<String>,
    round: Option<i64>,
    round_kills: i64,
    kills: i64,
    deaths: i64,
    assists: i64,
    mvps: i64,
    round_phase: Option<String>,
    bomb: Option<String>,
    map_phase: Option<String>,
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

impl Cs2Tracker {
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            initialised: false,
            my_team: None,
            round: None,
            round_kills: 0,
            kills: 0,
            deaths: 0,
            assists: 0,
            mvps: 0,
            round_phase: None,
            bomb: None,
            map_phase: None,
        }
    }

    /// Feed one GSI payload received at `rec_elapsed_ms` into the recording.
    pub fn ingest(&mut self, payload: &Value, rec_elapsed_ms: u64) -> Vec<GameEvent> {
        if payload["auth"]["token"].as_str() != Some(self.token.as_str()) {
            return vec![];
        }
        let mut out = Vec::new();
        let mk = |k: &str, meta: Value| GameEvent::new(k, rec_elapsed_ms, EventSource::Api { name: SOURCE_NAME.into() }).with_meta(meta);
        let map_round = int(&payload["map"]["round"]);
        // GSI counts rounds from 0; markers use 1-based round numbers.
        let round_no = map_round.map(|r| r + 1);

        // Player block: only when it is me (not a spectated teammate).
        let me = payload["provider"]["steamid"].as_str();
        let player = &payload["player"];
        let own = me.is_some() && player["steamid"].as_str() == me;
        if own {
            if let Some(team) = player["team"].as_str() {
                self.my_team = Some(team.to_string());
            }
            let ms = &player["match_stats"];
            let rk = int(&player["state"]["round_kills"]);
            let (k, d, a, m) = (int(&ms["kills"]), int(&ms["deaths"]), int(&ms["assists"]), int(&ms["mvps"]));
            if self.initialised {
                let new_round = map_round.is_some() && map_round != self.round;
                let prev_rk = if new_round { 0 } else { self.round_kills };
                let by_round = rk.map(|r| r - prev_rk).unwrap_or(0).max(0);
                let by_match = k.map(|k| k - self.kills).unwrap_or(0).max(0);
                for _ in 0..by_round.max(by_match) {
                    out.push(mk(kind::KILL, json!({ "round": round_no })));
                }
                for _ in 0..d.map(|d| d - self.deaths).unwrap_or(0).max(0) {
                    out.push(mk(kind::DEATH, json!({ "round": round_no })));
                }
                for _ in 0..a.map(|a| a - self.assists).unwrap_or(0).max(0) {
                    out.push(mk(kind::ASSIST, json!({ "round": round_no })));
                }
                if m.map(|m| m > self.mvps).unwrap_or(false) {
                    out.push(mk(kind::MVP, json!({ "round": round_no })));
                }
            }
            self.round_kills = rk.unwrap_or(0);
            self.kills = k.unwrap_or(self.kills);
            self.deaths = d.unwrap_or(self.deaths);
            self.assists = a.unwrap_or(self.assists);
            self.mvps = m.unwrap_or(self.mvps);
            if map_round.is_some() {
                self.round = map_round;
            }
            self.initialised = true;
        }

        // Round and map state are the same whoever we are watching.
        let round = &payload["round"];
        if let Some(phase) = round["phase"].as_str() {
            let prev = self.round_phase.as_deref();
            // A round begins with freeze time; if we missed it, with the switch to live.
            let starts = (phase == "freezetime" && prev != Some("freezetime")) || (phase == "live" && matches!(prev, None | Some("over")));
            if starts && self.map_phase.as_deref() != Some("warmup") && payload["map"]["phase"].as_str() != Some("warmup") {
                out.push(mk(kind::ROUND_START, json!({ "round": round_no })));
            }
            if phase == "over" && self.round_phase.as_deref() != Some("over") {
                if let (Some(win), Some(team)) = (round["win_team"].as_str(), self.my_team.as_deref()) {
                    let k = if win.eq_ignore_ascii_case(team) {
                        kind::ROUND_WIN
                    } else {
                        kind::ROUND_LOSS
                    };
                    out.push(mk(k, json!({ "round": round_no, "winTeam": win })));
                }
            }
            self.round_phase = Some(phase.to_string());
        }
        let bomb = round["bomb"].as_str().map(str::to_string);
        if bomb != self.bomb {
            match bomb.as_deref() {
                Some("planted") => out.push(mk(kind::BOMB_PLANT, json!({ "round": round_no }))),
                Some("defused") => out.push(mk(kind::BOMB_DEFUSE, json!({ "round": round_no }))),
                _ => {}
            }
            self.bomb = bomb;
        }
        if let Some(phase) = payload["map"]["phase"].as_str() {
            if phase == "live" && self.map_phase.as_deref() != Some("live") {
                out.push(mk(kind::MATCH_START, json!({ "map": payload["map"]["name"] })));
            }
            if phase == "gameover" && self.map_phase.as_deref() != Some("gameover") && self.map_phase.is_some() {
                out.push(mk(kind::MATCH_END, json!({ "map": payload["map"]["name"] })));
            }
            self.map_phase = Some(phase.to_string());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "76561198000000001";
    const MATE: &str = "76561198000000002";

    #[allow(clippy::too_many_arguments)]
    fn payload(
        player: &str,
        team: &str,
        round: i64,
        rk: i64,
        k: i64,
        d: i64,
        a: i64,
        mvps: i64,
        round_phase: &str,
        bomb: Option<&str>,
        map_phase: &str,
    ) -> Value {
        let mut r = json!({ "phase": round_phase });
        if let Some(b) = bomb {
            r["bomb"] = json!(b);
        }
        if round_phase == "over" {
            r["win_team"] = json!("CT");
        }
        json!({
            "provider": { "name": "Counter-Strike 2", "appid": 730, "steamid": ME },
            "map": { "name": "de_dust2", "phase": map_phase, "round": round },
            "round": r,
            "player": { "steamid": player, "team": team, "state": { "health": 100, "round_kills": rk }, "match_stats": { "kills": k, "deaths": d, "assists": a, "mvps": mvps } },
            "auth": { "token": "secret" }
        })
    }

    fn kinds(v: &[GameEvent]) -> Vec<&str> {
        v.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn tracks_my_events() {
        let mut t = Cs2Tracker::new("secret");
        // First payload (joining a live match): no stat deltas, but the round and match begin.
        let first = t.ingest(&payload(ME, "CT", 3, 1, 5, 2, 1, 0, "live", None, "live"), 0);
        assert_eq!(kinds(&first), vec!["round_start", "match_start"]);
        assert_eq!(first[0].meta["round"], 4, "GSI round 3 is the 4th round");
        // Two kills in a single update.
        assert_eq!(
            kinds(&t.ingest(&payload(ME, "CT", 3, 3, 7, 2, 1, 0, "live", None, "live"), 1000)),
            vec!["kill", "kill"]
        );
        // Bomb planted.
        assert_eq!(
            kinds(&t.ingest(&payload(ME, "CT", 3, 3, 7, 2, 1, 0, "live", Some("planted"), "live"), 2000)),
            vec!["bomb_plant"]
        );
        // I die; spectating a teammate: their stats must be ignored, round state still counts.
        assert_eq!(
            kinds(&t.ingest(&payload(ME, "CT", 3, 3, 7, 3, 1, 0, "live", Some("planted"), "live"), 3000)),
            vec!["death"]
        );
        assert!(t
            .ingest(&payload(MATE, "CT", 3, 4, 20, 0, 0, 0, "live", Some("planted"), "live"), 4000)
            .is_empty());
        assert_eq!(
            kinds(&t.ingest(&payload(MATE, "CT", 3, 4, 21, 0, 0, 0, "over", Some("defused"), "live"), 5000)),
            vec!["round_win", "bomb_defuse"]
        );
        // New round: freeze time starts it; round_kills resets; one kill, an assist and an MVP.
        let fresh = t.ingest(&payload(ME, "CT", 4, 0, 7, 3, 1, 0, "freezetime", None, "live"), 6000);
        assert_eq!(kinds(&fresh), vec!["round_start"]);
        assert_eq!(fresh[0].meta["round"], 5);
        let ev = t.ingest(&payload(ME, "CT", 4, 1, 8, 3, 2, 1, "live", None, "live"), 7000);
        assert_eq!(kinds(&ev), vec!["kill", "assist", "mvp"]);
        assert_eq!(ev[0].t_ms, 7000);
        assert_eq!(ev[0].meta["round"], 5);
        // Game over.
        assert_eq!(
            kinds(&t.ingest(&payload(ME, "CT", 4, 1, 8, 3, 2, 1, "over", None, "gameover"), 8000)),
            vec!["round_win", "match_end"]
        );
    }

    #[test]
    fn rejects_wrong_token() {
        let mut t = Cs2Tracker::new("other");
        assert!(t.ingest(&payload(ME, "CT", 1, 0, 0, 0, 0, 0, "live", None, "live"), 0).is_empty());
        let mut p = payload(ME, "CT", 1, 5, 5, 0, 0, 0, "live", Some("planted"), "live");
        p["auth"] = json!({});
        assert!(t.ingest(&p, 0).is_empty());
    }

    #[test]
    fn config_and_library_discovery() {
        let cfg = gsi_config(31337, "to\"ken");
        assert!(cfg.contains("\"uri\"       \"http://127.0.0.1:31337\""));
        assert!(cfg.contains("\"token\" \"token\""));
        assert_eq!(vdf_tokens(&cfg).iter().filter(|t| *t == "1").count(), 7);

        let d = tempfile::tempdir().unwrap();
        let steam = d.path().join("Steam");
        let other = d.path().join("Lib 2");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        let cfg_dir = other.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg");
        fs::create_dir_all(&cfg_dir).unwrap();
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\" {{ \"730\" \"123\" }}\n\t}}\n}}\n",
            steam.to_string_lossy().replace('\\', "\\\\"),
            other.to_string_lossy().replace('\\', "\\\\")
        );
        fs::write(steam.join("steamapps/libraryfolders.vdf"), vdf).unwrap();
        assert_eq!(steam_libraries(&steam).len(), 2);
        assert_eq!(find_cs2_cfg_dirs(&steam), vec![cfg_dir]);
    }
}
