//! Optional official local sources: League Live Client Data, CS2 GSI and the Riot Client's
//! local API (VALORANT details). All of them are on 127.0.0.1.

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
                        Some(s) if s.match_id == match_id => s.started.map(|_| s.elapsed_ms()),
                        _ => break,
                    }
                };
                // Before the first frame there is nothing to put events on.
                let Some(elapsed) = elapsed else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
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
                // Match on/off for per-match recording (only for our own config's token).
                if payload["auth"]["token"].as_str() == Some(token.as_str()) {
                    crate::matchwatch::observe_cs2(&app, &payload);
                }
                let st = state(&app);
                let (match_id, events) = {
                    let mut guard = st.session.lock().unwrap();
                    let Some(s) = guard.as_mut() else { continue };
                    let elapsed = s.elapsed_ms();
                    let started = s.started.is_some();
                    let Some(tracker) = s.cs2.as_mut() else { continue };
                    // Always ingest (keeps the tracker's baseline current) but only keep
                    // events once the recording has its first frame.
                    let events = tracker.ingest(&payload, elapsed);
                    (s.match_id.clone(), if started { events } else { Vec::new() })
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

// ---------------------------------------------------------------------------------------------
// Rocket League Stats API

/// What Recap knows about Rocket League's Stats API on this computer (shown on the Games page).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RlApiStatus {
    /// Install folders found (Epic, Steam, the running game).
    pub installs: Vec<String>,
    /// `PacketSendRate` > 0 in at least one install's `DefaultStatsAPI.ini`.
    pub enabled: bool,
    /// Recap switched it on while the game was running: it takes effect after a restart.
    pub needs_restart: bool,
    /// The ini could not be written (the install is in a protected folder).
    pub needs_admin: bool,
    pub port: u16,
    /// The game is streaming to Recap right now.
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

static RL_STATUS: std::sync::Mutex<Option<RlApiStatus>> = std::sync::Mutex::new(None);
static RL_EXE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
static RL_RUNNING: AtomicBool = AtomicBool::new(false);

/// The process monitor saw `RocketLeague.exe` at `exe` (called every second while it runs).
pub fn rocket_league_seen(exe: &std::path::Path) {
    RL_RUNNING.store(true, Ordering::Relaxed);
    let mut cur = RL_EXE.lock().unwrap();
    if cur.as_deref() != Some(exe) {
        *cur = Some(exe.to_path_buf());
    }
}

pub fn rocket_league_status() -> RlApiStatus {
    RL_STATUS.lock().unwrap().clone().unwrap_or_default()
}

fn rl_installs() -> Vec<PathBuf> {
    use recap_core::rocket_league::{epic_installs, install_from_exe, steam_installs};
    let mut v = Vec::new();
    if let Some(exe) = RL_EXE.lock().unwrap().clone() {
        v.extend(install_from_exe(&exe));
    }
    #[cfg(windows)]
    {
        let pd = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        v.extend(epic_installs(&pd.join(r"Epic\EpicGamesLauncher\Data\Manifests")));
    }
    #[cfg(not(windows))]
    let _ = epic_installs;
    for root in steam_roots() {
        v.extend(steam_installs(&root));
    }
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| p.is_dir() && seen.insert(p.to_string_lossy().to_lowercase()));
    v
}

/// Make sure every install has the Stats API on. `elevate` retries a protected install through
/// an administrator prompt (only when the player asked for it).
pub fn enable_rocket_league_api(elevate: bool) -> RlApiStatus {
    use recap_core::rocket_league::{parse_ini, stats_ini_path, with_rate, PACKET_RATE};
    let installs = rl_installs();
    let mut st = RlApiStatus {
        installs: installs.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        port: recap_core::rocket_league::DEFAULT_PORT,
        connected: rocket_league_status().connected,
        ..Default::default()
    };
    for inst in &installs {
        let path = stats_ini_path(inst);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let ini = parse_ini(&text);
        st.port = ini.port();
        if ini.enabled() {
            st.enabled = true;
            continue;
        }
        let new = with_rate(&text, PACKET_RATE);
        let wrote = std::fs::create_dir_all(path.parent().unwrap_or(inst)).and_then(|_| std::fs::write(&path, &new));
        match wrote {
            Ok(()) => {
                log::info!("Rocket League Stats API switched on in {}", path.display());
                st.enabled = true;
                st.needs_restart |= RL_RUNNING.load(Ordering::Relaxed);
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && elevate => match write_elevated(&path, &new) {
                Ok(()) => {
                    st.enabled = true;
                    st.needs_restart |= RL_RUNNING.load(Ordering::Relaxed);
                }
                Err(e) => {
                    st.needs_admin = true;
                    st.error = Some(format!("{e:#}"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => st.needs_admin = true,
            Err(e) => st.error = Some(format!("could not write {}: {e}", path.display())),
        }
    }
    if st.enabled {
        st.needs_admin = false;
    }
    *RL_STATUS.lock().unwrap() = Some(st.clone());
    st
}

/// Copy `text` over `path` from an elevated PowerShell (one administrator prompt).
fn write_elevated(path: &std::path::Path, text: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let tmp = std::env::temp_dir().join("recap-DefaultStatsAPI.ini");
        std::fs::write(&tmp, text)?;
        let q = |p: &std::path::Path| p.to_string_lossy().replace('\'', "''");
        let inner = format!("Copy-Item -LiteralPath '{}' -Destination '{}' -Force", q(&tmp), q(path));
        let script = format!(
            "$p = Start-Process powershell -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList '-NoProfile','-Command',\"{}\"; exit $p.ExitCode",
            inner.replace('"', "`\"")
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status()?;
        let _ = std::fs::remove_file(&tmp);
        anyhow::ensure!(status.success(), "the administrator prompt was declined");
        anyhow::ensure!(
            recap_core::rocket_league::parse_ini(&std::fs::read_to_string(path).unwrap_or_default()).enabled(),
            "the setting did not change"
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, text);
        anyhow::bail!("needs Windows")
    }
}

/// Follow the Stats API whenever Rocket League runs: match on/off for per-match recording, and
/// plays and details into the running recording.
pub fn start_rocket_league(app: AppHandle) {
    std::thread::Builder::new()
        .name("rocket-league".into())
        .spawn(move || rl_loop(app))
        .ok();
}

fn rl_loop(app: AppHandle) {
    use recap_core::{games::ApiSource, rocket_league::RlTracker};
    let epoch = std::time::Instant::now();
    let now_ms = move || epoch.elapsed().as_millis() as u64;
    let mut tracker = RlTracker::new();
    let mut last_setup: Option<std::time::Instant> = None;
    let mut was_running = false;
    loop {
        let ids = crate::matchwatch::games_with(&app, ApiSource::RocketLeagueStats);
        let running = RL_RUNNING.swap(false, Ordering::Relaxed);
        if ids.is_empty() {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        // Switch the API on at startup, when the game starts, and every few minutes.
        if last_setup.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) || (running && !was_running) {
            enable_rocket_league_api(false);
            last_setup = Some(std::time::Instant::now());
        }
        was_running = running;
        let status = rocket_league_status();
        if !running {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        if !status.enabled || status.needs_restart {
            // No signal yet: record whole sessions (split afterwards), as before.
            ids.iter().for_each(|id| crate::matchwatch::clear(&app, id));
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        match std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], status.port).into(), Duration::from_secs(1)) {
            Ok(stream) => {
                set_connected(true);
                follow_stream(&app, &ids, stream, &mut tracker, &now_ms);
                set_connected(false);
            }
            Err(_) => {
                // The game is up but serves nothing: menus, free play, or it was not restarted
                // since the setting changed. Not in a match.
                for id in &ids {
                    crate::matchwatch::observe(&app, id, "rocket-league-stats", false);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

fn set_connected(on: bool) {
    let mut s = RL_STATUS.lock().unwrap();
    s.get_or_insert_with(Default::default).connected = on;
}

/// Read one connection until the game closes it.
fn follow_stream(
    app: &AppHandle,
    ids: &[String],
    mut stream: std::net::TcpStream,
    tracker: &mut recap_core::rocket_league::RlTracker,
    now_ms: &dyn Fn() -> u64,
) {
    use std::io::Write;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut framer = recap_core::rocket_league::JsonStream::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got_any = false;
    let mut tried_ws = false;
    let opened = std::time::Instant::now();
    let mut recording: Option<String> = None;
    let mut last_observe = std::time::Instant::now() - Duration::from_secs(10);
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => 0,
            Err(_) => break,
        };
        // Some builds may speak WebSocket: if nothing arrives, ask for an upgrade once. The
        // framer skips the HTTP reply and frame headers and still finds the JSON objects.
        if n == 0 && !got_any && !tried_ws && opened.elapsed() > Duration::from_secs(3) {
            tried_ws = true;
            let req = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: cmVjYXAtcm9ja2V0LWxlYWd1ZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
            let _ = stream.write_all(req.as_bytes());
        }
        got_any |= n > 0;
        let msgs = framer.push(&buf[..n]);
        let now = now_ms();
        let st = state(app);
        for msg in msgs {
            // Which recording (if any) these plays belong to, and where it is.
            let (match_id, rec_ms) = {
                let guard = st.session.lock().unwrap();
                match guard.as_ref() {
                    Some(s) if s.started.is_some() && s.game.as_ref().is_some_and(|g| ids.contains(&g.def.id)) => {
                        (Some(s.match_id.clone()), Some(s.elapsed_ms()))
                    }
                    _ => (None, None),
                }
            };
            if match_id != recording {
                tracker.new_recording();
                recording = match_id.clone();
            }
            let events = tracker.ingest(&msg, now, rec_ms);
            if let Some(id) = &match_id {
                add_live_events(app, id, events);
            }
        }
        if last_observe.elapsed() >= Duration::from_secs(1) {
            let on = tracker.in_match(now);
            for id in ids {
                crate::matchwatch::observe(app, id, "rocket-league-stats", on);
            }
            last_observe = std::time::Instant::now();
        }
    }
}
