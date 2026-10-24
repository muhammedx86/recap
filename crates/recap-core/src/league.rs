//! League of Legends / TFT: the official Live Client Data API.
//!
//! While a game is running, the client serves `allgamedata` on
//! `https://127.0.0.1:2999` (self-signed certificate). The app polls it about
//! once a second and feeds each response to [`LeagueTracker::ingest`], which
//! turns new entries of `events.Events` into [`GameEvent`]s for the local player.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use crate::{
    details::{field, normalize_result, SnapshotEmitter},
    events::{kind, EventSource, GameEvent},
};

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
    details: SnapshotEmitter,
    result: Option<String>,
}

/// Map number -> name (Live Client `gameData.mapNumber`).
fn map_name(n: i64) -> Option<&'static str> {
    Some(match n {
        11 => "Summoner's Rift",
        12 => "Howling Abyss",
        21 => "Nexus Blitz",
        22 => "Convergence",
        30 => "Rings of Wrath",
        _ => return None,
    })
}

fn mode_name(m: &str) -> String {
    match m {
        "CLASSIC" => "Classic".into(),
        "ARAM" => "ARAM".into(),
        "CHERRY" => "Arena".into(),
        "URF" => "URF".into(),
        "PRACTICETOOL" => "Practice Tool".into(),
        "TUTORIAL" => "Tutorial".into(),
        other => crate::details::pretty_name(other),
    }
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

    /// Current details: champion, map, mode, K/D/A, result.
    fn snapshot(&self, all: &Value) -> BTreeMap<String, String> {
        let mut d = BTreeMap::new();
        let me = all["allPlayers"].as_array().and_then(|ps| {
            ps.iter().find(|p| {
                ["riotId", "riotIdGameName", "summonerName"]
                    .iter()
                    .any(|k| self.is_me(p[*k].as_str()))
            })
        });
        if let Some(p) = me {
            if let Some(c) = p["championName"].as_str() {
                d.insert(field::CHARACTER.to_string(), c.to_string());
            }
            let sc = &p["scores"];
            for (f, k) in [(field::KILLS, "kills"), (field::DEATHS, "deaths"), (field::ASSISTS, "assists")] {
                if let Some(n) = sc[k].as_i64() {
                    d.insert(f.to_string(), n.to_string());
                }
            }
        }
        let gd = &all["gameData"];
        if let Some(m) = gd["mapNumber"].as_i64().and_then(map_name) {
            d.insert(field::MAP.to_string(), m.to_string());
        }
        if let Some(m) = gd["gameMode"].as_str() {
            d.insert(field::MODE.to_string(), mode_name(m));
        }
        if let Some(r) = &self.result {
            d.insert(field::RESULT.to_string(), r.clone());
        }
        d
    }

    /// Feed one `allgamedata` response. `rec_elapsed_ms` is the recording time when it was received.
    pub fn ingest(&mut self, all: &Value, rec_elapsed_ms: u64) -> Vec<GameEvent> {
        let mut out = self.ingest_events(all, rec_elapsed_ms);
        let snap = self.snapshot(all);
        out.extend(self.details.update(snap, rec_elapsed_ms, SOURCE_NAME));
        out
    }

    fn ingest_events(&mut self, all: &Value, rec_elapsed_ms: u64) -> Vec<GameEvent> {
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
                "GameEnd" => {
                    if let Some(r) = ev["Result"].as_str().and_then(normalize_result) {
                        self.result = Some(r.to_string());
                    }
                    out.push(mk(kind::MATCH_END, json!({ "result": ev["Result"] })))
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::details::{latest_snapshot, SNAPSHOT_KIND};

    fn ingest_ev(t: &mut LeagueTracker, all: &Value, ms: u64) -> Vec<GameEvent> {
        t.ingest(all, ms).into_iter().filter(|e| e.kind != SNAPSHOT_KIND).collect()
    }

    #[test]
    fn details_snapshot() {
        let mut t = LeagueTracker::new();
        let mut d = data(300.0, json!([]));
        d["allPlayers"] = json!([
            { "riotId": "Other#EUW", "championName": "Zed", "scores": { "kills": 9, "deaths": 0, "assists": 0 } },
            { "riotId": "Faker#KR1", "championName": "Ahri", "scores": { "kills": 4, "deaths": 1, "assists": 7 } }
        ]);
        d["gameData"]["mapNumber"] = json!(11);
        let ev = t.ingest(&d, 1000);
        assert!(t.ingest(&d, 2000).is_empty(), "unchanged snapshot is not re-sent");
        d["events"]["Events"] = json!([{ "EventID": 9, "EventName": "GameEnd", "EventTime": 300.0, "Result": "Win" }]);
        let mut all = ev;
        all.extend(t.ingest(&d, 3000));
        let det = latest_snapshot(&all);
        let v = |k: &str| det.get(k).map(|v| v.value.as_str()).unwrap_or("");
        assert_eq!((v("character"), v("map"), v("mode")), ("Ahri", "Summoner's Rift", "Classic"));
        assert_eq!((v("kills"), v("deaths"), v("assists"), v("result")), ("4", "1", "7", "win"));
    }

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
        let ev = ingest_ev(&mut t, &first, 10_000);
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
        let ev = ingest_ev(&mut t, &second, 30_000);
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
        assert!(ingest_ev(&mut t, &json!({}), 0).is_empty());
        assert!(ingest_ev(&mut t, &json!({ "gameData": { "gameTime": 5.0 } }), 0).is_empty());
        // No identity known -> nothing is attributed to me.
        let ev = ingest_ev(
            &mut t,
            &json!({ "gameData": { "gameTime": 6.0 }, "events": { "Events": [ { "EventID": 1, "EventName": "ChampionKill", "EventTime": 5.5, "KillerName": "X", "VictimName": "Y" } ] } }),
            1000,
        );
        assert!(ev.is_empty());
    }
}
