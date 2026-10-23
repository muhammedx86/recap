//! League of Legends / TFT: the official Live Client Data API.
//!
//! While a game is running, the client serves `allgamedata` on
//! `https://127.0.0.1:2999` (self-signed certificate). The app polls it about
//! once a second and feeds each response to [`LeagueTracker::ingest`], which
//! turns new entries of `events.Events` into [`GameEvent`]s for the local player.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::events::{kind, EventSource, GameEvent};

pub const LIVE_CLIENT_ALLGAMEDATA: &str = "https://127.0.0.1:2999/liveclientdata/allgamedata";
pub const SOURCE_NAME: &str = "league-live-client";

/// Events that happened more than this long before recording began are ignored.
const MAX_BACKFILL_MS: u64 = 30_000;

#[derive(Debug, Default)]
pub struct LeagueTracker {
    seen: HashSet<i64>,
    /// Lowercased identities of the local player (riot id, game name, summoner name).
    me: HashSet<String>,
    /// Game time (seconds) at which recording began, learned on the first ingest.
    recording_start_game_s: Option<f64>,
}

fn norm(name: &str) -> String {
    name.trim().to_lowercase()
}

/// "Name#TAG" -> "name"
fn base_name(name: &str) -> String {
    norm(name.split('#').next().unwrap_or(name))
}

impl LeagueTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn learn_identity(&mut self, all: &Value) {
        let ap = &all["activePlayer"];
        let mut ids = HashSet::new();
        for key in ["riotId", "riotIdGameName", "summonerName"] {
            if let Some(s) = ap[key].as_str().filter(|s| !s.trim().is_empty()) {
                ids.insert(norm(s));
                ids.insert(base_name(s));
            }
        }
        if !ids.is_empty() {
            self.me = ids;
        }
    }

    fn is_me(&self, name: Option<&str>) -> bool {
        match name {
            Some(n) if !n.trim().is_empty() => self.me.contains(&norm(n)) || self.me.contains(&base_name(n)),
            _ => false,
        }
    }

    fn me_assisted(&self, ev: &Value) -> bool {
        ev["Assisters"].as_array().is_some_and(|a| a.iter().any(|x| self.is_me(x.as_str())))
    }

    /// Feed one `allgamedata` response. `rec_elapsed_ms` is the recording time when it was received.
    pub fn ingest(&mut self, all: &Value, rec_elapsed_ms: u64) -> Vec<GameEvent> {
        self.learn_identity(all);
        let Some(game_time) = all["gameData"]["gameTime"].as_f64() else {
            return vec![];
        };
        let start = *self
            .recording_start_game_s
            .get_or_insert(game_time - rec_elapsed_ms as f64 / 1000.0);
        let Some(list) = all["events"]["Events"].as_array() else {
            return vec![];
        };

        let mut out = Vec::new();
        for ev in list {
            let Some(id) = ev["EventID"].as_i64() else { continue };
            if !self.seen.insert(id) {
                continue;
            }
            let ev_time = ev["EventTime"].as_f64().unwrap_or(game_time);
            if (start - ev_time) * 1000.0 > MAX_BACKFILL_MS as f64 {
                continue;
            }
            let t_ms = (rec_elapsed_ms as f64 - (game_time - ev_time) * 1000.0).max(0.0).round() as u64;
            let mk = |k: &str, meta: Value| GameEvent::new(k, t_ms, EventSource::Api { name: SOURCE_NAME.into() }).with_meta(meta);
            let killer = ev["KillerName"].as_str();
            match ev["EventName"].as_str().unwrap_or_default() {
                "ChampionKill" => {
                    let victim = ev["VictimName"].as_str();
                    if self.is_me(killer) {
                        out.push(mk(kind::KILL, json!({ "victim": victim })));
                    }
                    if self.is_me(victim) {
                        out.push(mk(kind::DEATH, json!({ "killer": killer })));
                    }
                    if self.me_assisted(ev) {
                        out.push(mk(kind::ASSIST, json!({ "victim": victim })));
                    }
                }
                "Multikill" if self.is_me(killer) => {
                    out.push(mk(kind::MULTIKILL, json!({ "count": ev["KillStreak"] })));
                }
                "Ace" if self.is_me(ev["Acer"].as_str()) => {
                    out.push(mk(kind::ACE, Value::Null));
                }
                name @ ("DragonKill" | "BaronKill" | "HeraldKill" | "HordeKill" | "TurretKilled" | "InhibKilled")
                    if self.is_me(killer) || self.me_assisted(ev) =>
                {
                    let mut meta = json!({ "type": name, "byMe": self.is_me(killer) });
                    if let Some(d) = ev["DragonType"].as_str() {
                        meta["dragon"] = json!(d);
                    }
                    if ev["Stolen"].as_str() == Some("True") {
                        meta["stolen"] = json!(true);
                    }
                    out.push(mk(kind::OBJECTIVE, meta));
                }
                "GameEnd" => out.push(mk(kind::MATCH_END, json!({ "result": ev["Result"] }))),
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(game_time: f64, events: Value) -> Value {
        json!({
            "activePlayer": { "riotId": "Faker#KR1", "riotIdGameName": "Faker", "riotIdTagLine": "KR1", "summonerName": "Faker#KR1", "level": 6 },
            "allPlayers": [],
            "events": { "Events": events },
            "gameData": { "gameMode": "CLASSIC", "gameTime": game_time, "mapName": "Map11" }
        })
    }

    #[test]
    fn maps_events_for_local_player() {
        let mut t = LeagueTracker::new();
        // Recording started at game time 100s (rec 10s in at gameTime 110).
        let first = data(
            110.0,
            json!([
                { "EventID": 0, "EventName": "GameStart", "EventTime": 0.03 },
                { "EventID": 1, "EventName": "ChampionKill", "EventTime": 95.0, "KillerName": "Faker", "VictimName": "Zed", "Assisters": [] },
                { "EventID": 2, "EventName": "ChampionKill", "EventTime": 105.0, "KillerName": "faker", "VictimName": "Ahri", "Assisters": ["Lee"] },
                { "EventID": 3, "EventName": "ChampionKill", "EventTime": 108.0, "KillerName": "Ahri", "VictimName": "Faker#KR1", "Assisters": [] }
            ]),
        );
        let ev = t.ingest(&first, 10_000);
        // GameStart is too old (skipped); kill at 95s is 5s before recording -> t=0.
        assert_eq!(
            ev.iter().map(|e| (e.kind.as_str(), e.t_ms)).collect::<Vec<_>>(),
            vec![("kill", 0), ("kill", 5_000), ("death", 8_000)]
        );
        assert_eq!(ev[1].meta["victim"], "Ahri");
        assert_eq!(ev[0].source, EventSource::Api { name: SOURCE_NAME.into() });

        // Second poll repeats old events plus new ones; only new ones come out.
        let second = data(
            130.0,
            json!([
                { "EventID": 2, "EventName": "ChampionKill", "EventTime": 105.0, "KillerName": "faker", "VictimName": "Ahri", "Assisters": [] },
                { "EventID": 4, "EventName": "ChampionKill", "EventTime": 125.0, "KillerName": "Lee", "VictimName": "Ahri", "Assisters": ["Faker"] },
                { "EventID": 5, "EventName": "Multikill", "EventTime": 126.0, "KillerName": "Faker", "KillStreak": 2 },
                { "EventID": 6, "EventName": "DragonKill", "EventTime": 127.0, "KillerName": "Lee", "Assisters": ["Faker"], "DragonType": "Earth", "Stolen": "False" },
                { "EventID": 7, "EventName": "TurretKilled", "EventTime": 128.0, "KillerName": "Minion_T100", "Assisters": [] },
                { "EventID": 8, "EventName": "Ace", "EventTime": 129.0, "Acer": "Faker", "AcingTeam": "ORDER" },
                { "EventID": 9, "EventName": "GameEnd", "EventTime": 129.5, "Result": "Win" }
            ]),
        );
        let ev = t.ingest(&second, 30_000);
        let got: Vec<_> = ev.iter().map(|e| (e.kind.as_str(), e.t_ms)).collect();
        assert_eq!(
            got,
            vec![
                ("assist", 25_000),
                ("multikill", 26_000),
                ("objective", 27_000),
                ("ace", 29_000),
                ("match_end", 29_500)
            ]
        );
        assert_eq!(ev[1].meta["count"], 2);
        assert_eq!(ev[2].meta["dragon"], "Earth");
        assert_eq!(ev[4].meta["result"], "Win");
    }

    #[test]
    fn tolerates_missing_fields() {
        let mut t = LeagueTracker::new();
        assert!(t.ingest(&json!({}), 0).is_empty());
        assert!(t.ingest(&json!({ "gameData": { "gameTime": 5.0 } }), 0).is_empty());
        // No identity known -> nothing is attributed to me.
        let ev = t.ingest(
            &json!({ "gameData": { "gameTime": 6.0 }, "events": { "Events": [ { "EventID": 1, "EventName": "ChampionKill", "EventTime": 5.5, "KillerName": "X", "VictimName": "Y" } ] } }),
            1000,
        );
        assert!(ev.is_empty());
    }
}
