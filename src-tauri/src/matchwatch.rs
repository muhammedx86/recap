//! Live "is a match on?" signals from game APIs, so a game that stays open records each match
//! separately (see `recap_core::matchgate`).
//!
//! * VALORANT: the Riot Client's local presence (`sessionLoopState` INGAME), polled every 2 s.
//!   The same loop records map/mode/score/result snapshots into the running recording.
//! * CS2: Game State Integration (`map.phase` live/intermission), fed from `sources`.
//! * Marvel Rivals: the game's `current_connect_info` state file (battle id while in a match).
//! * League of Legends / TFT need nothing: the game process only exists during a game.
//!
//! Games without such a signal record the whole session and are split afterwards.

use std::time::{Duration, Instant};

use recap_core::{
    games::ApiSource,
    matchgate::MatchGate,
    riot_local::{decode_private, lockfile_path, Lockfile, RiotTracker},
};
use tauri::AppHandle;

use crate::state::{add_live_events, state};

/// Keep recording this long after a match ends (end-of-match screens).
pub const TAIL_MS: u64 = 4_000;

/// A signal older than this is ignored (the game or its API went away): the game falls back to
/// recording whole sessions.
const STALE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct GateEntry {
    pub gate: MatchGate,
    pub updated: Instant,
}

impl GateEntry {
    pub fn fresh(&self) -> bool {
        self.updated.elapsed() < STALE
    }
}

fn now_ms() -> u64 {
    crate::state::now_unix_ms()
}

/// Feed one in-match observation for `game_id`.
pub fn observe(app: &AppHandle, game_id: &str, source: &'static str, in_match: bool) {
    let st = state(app);
    let mut gates = st.gates.lock().unwrap();
    let new = !gates.contains_key(game_id);
    let e = gates.entry(game_id.to_string()).or_insert_with(|| GateEntry {
        gate: MatchGate::new(TAIL_MS),
        updated: Instant::now(),
    });
    let was = e.gate.in_match();
    e.gate.observe(in_match, now_ms());
    e.updated = Instant::now();
    drop(gates);
    if was != in_match || new {
        log::info!("{game_id}: {} ({source})", if in_match { "in a match" } else { "not in a match" });
        crate::state::refresh_status(app, None);
    }
}

/// Forget a game's signal (its API is gone).
pub fn clear(app: &AppHandle, game_id: &str) {
    let removed = state(app).gates.lock().unwrap().remove(game_id).is_some();
    if removed {
        crate::state::refresh_status(app, None);
    }
}

/// Ids of enabled games using `api`.
fn games_with(app: &AppHandle, api: ApiSource) -> Vec<String> {
    let st = state(app);
    let settings = st.settings.read().unwrap();
    let ids = st
        .games
        .read()
        .unwrap()
        .iter()
        .filter(|g| g.def.api == Some(api) && settings.game_enabled(&g.def.id))
        .map(|g| g.def.id.clone())
        .collect();
    ids
}

/// Watch the Riot Client for VALORANT matches (localhost only).
pub fn spawn_riot_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("riot-watcher".into())
        .spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                // The Riot Client serves a self-signed certificate on localhost.
                .danger_accept_invalid_certs(true)
                .timeout(Duration::from_secs(2))
                .no_proxy()
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("riot watcher: {e}");
                    return;
                }
            };
            let mut puuid: Option<(u16, String)> = None;
            // Details for the recording in progress (reset per recording).
            let mut tracker: Option<(String, RiotTracker)> = None;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let ids = games_with(&app, ApiSource::RiotClient);
                if ids.is_empty() {
                    continue;
                }
                let Some(lock) = lockfile_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|t| Lockfile::parse(&t))
                else {
                    ids.iter().for_each(|id| clear(&app, id));
                    continue;
                };
                let get = |path: &str| {
                    client
                        .get(format!("{}{path}", lock.base_url()))
                        .header("Authorization", lock.auth_header())
                        .send()
                        .and_then(|r| r.json::<serde_json::Value>())
                };
                if puuid.as_ref().is_none_or(|(port, _)| *port != lock.port) {
                    puuid = get("/chat/v1/session")
                        .ok()
                        .and_then(|v| v["puuid"].as_str().map(|p| (lock.port, p.to_string())));
                }
                let Some((_, me)) = puuid.as_ref() else { continue };
                let private = get("/chat/v4/presences").ok().and_then(|p| decode_private(&p, me));
                let Some(private) = private else {
                    // The Riot Client runs but VALORANT does not: no signal.
                    ids.iter().for_each(|id| clear(&app, id));
                    continue;
                };
                let in_match = find_str(&private, "sessionLoopState").is_some_and(|s| s.eq_ignore_ascii_case("INGAME"));
                for id in &ids {
                    observe(&app, id, "riot-client", in_match);
                }
                // Details snapshots into the current VALORANT recording.
                let st = state(&app);
                let current = st
                    .session
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|s| (ids.contains(&s.game_id()) && s.started.is_some()).then(|| (s.match_id.clone(), s.elapsed_ms())));
                if let Some((match_id, elapsed)) = current {
                    if tracker.as_ref().is_none_or(|(id, _)| *id != match_id) {
                        tracker = Some((match_id.clone(), RiotTracker::new()));
                    }
                    if let Some((_, t)) = tracker.as_mut() {
                        add_live_events(&app, &match_id, t.ingest(&private, elapsed));
                    }
                }
            }
        })
        .ok();
}

fn find_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    match v {
        serde_json::Value::Object(m) => m
            .get(key)
            .and_then(|x| x.as_str())
            .or_else(|| m.values().find_map(|c| find_str(c, key))),
        serde_json::Value::Array(a) => a.iter().find_map(|c| find_str(c, key)),
        _ => None,
    }
}

/// Watch Marvel Rivals' state file for matches (like Ascent): it holds a battle id while a match is
/// on and is cleared afterwards.
pub fn spawn_marvel_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("marvel-watcher".into())
        .spawn(move || {
            let mut last_battle: Option<String> = None;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let ids = games_with(&app, ApiSource::MarvelStateFile);
                if ids.is_empty() {
                    continue;
                }
                let Some(dir) =
                    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join(r"Marvel\Saved\HotTemp\stateinfo"))
                else {
                    continue;
                };
                if !dir.is_dir() {
                    // The game never ran here (or moved the file): no signal, record whole sessions.
                    ids.iter().for_each(|id| clear(&app, id));
                    continue;
                }
                let battle = std::fs::read_to_string(dir.join("current_connect_info"))
                    .ok()
                    .and_then(|c| recap_core::matchgate::marvel_battle_id(&c));
                if battle.is_some() && last_battle.is_some() && battle != last_battle {
                    // Straight into another match: end the previous one first.
                    ids.iter().for_each(|id| observe(&app, id, "marvel-state-file", false));
                }
                for id in &ids {
                    observe(&app, id, "marvel-state-file", battle.is_some());
                }
                last_battle = battle;
            }
        })
        .ok();
}

/// CS2: a GSI payload arrived. Live rounds and intermissions count as "in a match".
pub fn observe_cs2(app: &AppHandle, payload: &serde_json::Value) {
    for id in games_with(app, ApiSource::Cs2Gsi) {
        let phase = payload["map"]["phase"].as_str();
        let in_match = matches!(phase, Some("live" | "intermission"));
        observe(app, &id, "cs2-gsi", in_match);
    }
}
