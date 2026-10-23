//! Optional official local event sources: League Live Client Data and CS2 GSI.

use std::{
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use recap_core::{
    cs2::{find_cs2_cfg_dirs, gsi_config, GSI_CFG_NAME},
    league::{LeagueTracker, LIVE_CLIENT_ALLGAMEDATA},
};
use tauri::AppHandle;

use crate::state::{add_live_events, state};

/// Poll the League Live Client Data API once a second while the match records.
pub fn spawn_league_poller(app: AppHandle, match_id: String, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("league-poller".into())
        .spawn(move || {
            let client = match reqwest::blocking::Client::builder()
                // The Live Client API uses Riot's self-signed certificate on localhost.
                .danger_accept_invalid_certs(true)
                .timeout(Duration::from_secs(2))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    log::error!("league poller: {e}");
                    return;
                }
            };
            let mut tracker = LeagueTracker::new();
            while !stop.load(Ordering::Relaxed) {
                let elapsed = {
                    let st = state(&app);
                    let guard = st.session.lock().unwrap();
                    match guard.as_ref() {
                        Some(s) if s.match_id == match_id => s.elapsed_ms(),
                        _ => break,
                    }
                };
                if let Ok(resp) = client.get(LIVE_CLIENT_ALLGAMEDATA).send() {
                    if let Ok(json) = resp.json::<serde_json::Value>() {
                        let events = tracker.ingest(&json, elapsed);
                        add_live_events(&app, &match_id, events);
                    }
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        })
        .ok();
}

/// Port the CS2 GSI listener uses.
pub const GSI_PORT: u16 = 47_821;

/// Install the GSI config into every CS2 install we can find, then serve updates.
pub fn start_cs2_gsi(app: AppHandle) {
    let token = state(&app).gsi_token.clone();
    let server = match tiny_http::Server::http(("127.0.0.1", GSI_PORT)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("CS2 GSI listener unavailable on port {GSI_PORT}: {e}");
            return;
        }
    };
    let cfg = gsi_config(GSI_PORT, &token);
    for root in steam_roots() {
        for dir in find_cs2_cfg_dirs(&root) {
            let path = dir.join(GSI_CFG_NAME);
            let current = std::fs::read_to_string(&path).unwrap_or_default();
            if current != cfg {
                match std::fs::write(&path, &cfg) {
                    Ok(_) => log::info!("installed CS2 GSI config at {}", path.display()),
                    Err(e) => log::warn!("could not write {}: {e}", path.display()),
                }
            }
        }
    }
    std::thread::Builder::new()
        .name("cs2-gsi".into())
        .spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().take(4 * 1024 * 1024).read_to_string(&mut body);
                let _ = req.respond(tiny_http::Response::empty(200));
                let Ok(payload) = serde_json::from_str::<serde_json::Value>(&body) else {
                    continue;
                };
                let st = state(&app);
                let (match_id, events) = {
                    let mut guard = st.session.lock().unwrap();
                    let Some(s) = guard.as_mut() else { continue };
                    let elapsed = s.elapsed_ms();
                    let Some(tracker) = s.cs2.as_mut() else { continue };
                    (s.match_id.clone(), tracker.ingest(&payload, elapsed))
                };
                add_live_events(&app, &match_id, events);
            }
        })
        .ok();
}

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam") {
            if let Ok(p) = key.get_value::<String, _>("SteamPath") {
                roots.push(PathBuf::from(p.replace('/', "\\")));
            }
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }
    #[cfg(not(windows))]
    {
        if let Some(h) = dirs::home_dir() {
            roots.push(h.join(".steam/steam"));
            roots.push(h.join(".local/share/Steam"));
        }
    }
    roots.dedup();
    roots.into_iter().filter(|p| p.exists()).collect()
}
