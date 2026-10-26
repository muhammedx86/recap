//! Official local sources, each on its own thread, all on 127.0.0.1. Adapted from Recap's
//! `src-tauri/src/sources.rs` (GPL-3.0-or-later, see NOTICE): same endpoints and trackers,
//! feeding a channel instead of Recap's recording session.
//!
//! Every event's `t_ms` is milliseconds since [`Clock`] started; `main` turns it into unix ms.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use recap_core::cs2::{find_cs2_cfg_dirs, gsi_config, Cs2Tracker, GSI_CFG_NAME};
use recap_core::events::GameEvent;
use recap_core::games::ApiSource;
use recap_core::league::{LeagueTracker, LIVE_CLIENT_ALLGAMEDATA};
use recap_core::riot_local::{decode_private, lockfile_path, riot_id, Lockfile, RiotTracker};
use recap_core::rocket_league::{install_from_exe, parse_ini, stats_ini_path, JsonStream, RlTracker};

/// Port the CS2 GSI listener uses (Recap's).
pub const GSI_PORT: u16 = 47_821;

/// One time base for all sources: an `Instant` for elapsed ms, and the unix ms it started at.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    start: Instant,
    start_unix_ms: u64,
}

impl Clock {
    pub fn new() -> Self {
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        Self { start: Instant::now(), start_unix_ms: unix }
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    pub fn to_unix(&self, elapsed_ms: u64) -> u64 {
        self.start_unix_ms + elapsed_ms
    }
    pub fn now_unix(&self) -> u64 {
        self.to_unix(self.elapsed_ms())
    }
}

/// Starts the source a game's data names. `exe` is the running game's full path, when known.
pub fn start(api: ApiSource, clock: Clock, tx: Sender<Vec<GameEvent>>, exe: Option<PathBuf>) {
    let name = format!("{api:?}");
    let spawned = std::thread::Builder::new().name(name).spawn(move || match api {
        ApiSource::LeagueLiveClient => league(clock, tx),
        ApiSource::Cs2Gsi => cs2(clock, tx),
        ApiSource::RiotClient => riot(clock, tx),
        ApiSource::RocketLeagueStats => rocket_league(clock, tx, exe),
        ApiSource::MarvelStateFile => log::info!("marvel-state-file only reports match start/end; nothing to forward"),
    });
    if let Err(e) = spawned {
        log::error!("could not start {api:?}: {e}");
    }
}

fn local_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        // Riot's local APIs use self-signed certificates on 127.0.0.1.
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| log::error!("http client: {e}"))
        .ok()
}

/// League / TFT Live Client Data API, polled once a second.
fn league(clock: Clock, tx: Sender<Vec<GameEvent>>) {
    let Some(client) = local_client() else { return };
    let mut tracker = LeagueTracker::new();
    loop {
        if let Ok(json) = client.get(LIVE_CLIENT_ALLGAMEDATA).send().and_then(|r| r.json::<serde_json::Value>()) {
            let events = tracker.ingest(&json, clock.elapsed_ms());
            if !events.is_empty() && tx.send(events).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// CS2 Game State Integration: installs the config, then serves the game's POSTs.
/// CS2 reads the config at launch, so the first run after installing it needs a game restart.
fn cs2(clock: Clock, tx: Sender<Vec<GameEvent>>) {
    let token = format!("{:x}{:x}", clock.start_unix_ms, std::process::id());
    let server = match tiny_http::Server::http(("127.0.0.1", GSI_PORT)) {
        Ok(s) => s,
        Err(e) => return log::warn!("CS2 GSI listener unavailable on port {GSI_PORT}: {e}"),
    };
    let cfg = gsi_config(GSI_PORT, &token);
    for root in steam_roots() {
        for dir in find_cs2_cfg_dirs(&root) {
            let path = dir.join(GSI_CFG_NAME);
            if std::fs::read_to_string(&path).unwrap_or_default() != cfg {
                match std::fs::write(&path, &cfg) {
                    Ok(()) => log::info!("installed CS2 GSI config at {} (takes effect when CS2 restarts)", path.display()),
                    Err(e) => log::warn!("could not write {}: {e}", path.display()),
                }
            }
        }
    }
    let mut tracker = Cs2Tracker::new(&token);
    for mut req in server.incoming_requests() {
        let mut body = String::new();
        let _ = req.as_reader().take(4 * 1024 * 1024).read_to_string(&mut body);
        let _ = req.respond(tiny_http::Response::empty(200));
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(&body) else { continue };
        let events = tracker.ingest(&payload, clock.elapsed_ms());
        if !events.is_empty() && tx.send(events).is_err() {
            return;
        }
    }
}

/// VALORANT through the Riot Client's local API: round won / lost from the live score.
fn riot(clock: Clock, tx: Sender<Vec<GameEvent>>) {
    let Some(client) = local_client() else { return };
    let Some(lockfile) = lockfile_path() else { return };
    let mut tracker = RiotTracker::new();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        // The client rewrites the lockfile (new port and password) when it restarts.
        let Some(lock) = std::fs::read_to_string(&lockfile).ok().and_then(|t| Lockfile::parse(&t)) else { continue };
        let get = |path: &str| {
            client
                .get(format!("{}{path}", lock.base_url()))
                .header("Authorization", lock.auth_header())
                .send()
                .and_then(|r| r.json::<serde_json::Value>())
                .ok()
        };
        let Some(session) = get("/chat/v1/session") else { continue };
        let Some(puuid) = session["puuid"].as_str().map(str::to_owned) else { continue };
        tracker.set_player(riot_id(&session));
        let Some(presences) = get("/chat/v4/presences") else { continue };
        let Some(private) = decode_private(&presences, &puuid) else { continue };
        let events = tracker.ingest(&private, clock.elapsed_ms());
        if !events.is_empty() && tx.send(events).is_err() {
            return;
        }
    }
}

/// Rocket League Stats API: JSON over TCP while a match runs. Needs `PacketSendRate` > 0 in
/// `DefaultStatsAPI.ini`; this exe only reads that file (Settings can turn it on).
fn rocket_league(clock: Clock, tx: Sender<Vec<GameEvent>>, exe: Option<PathBuf>) {
    let Some(install) = exe.as_deref().and_then(install_from_exe) else {
        return log::warn!("Rocket League: install folder unknown (pass --exe)");
    };
    let ini = parse_ini(&std::fs::read_to_string(stats_ini_path(&install)).unwrap_or_default());
    if !ini.enabled() {
        return log::warn!("Rocket League: Stats API is off in {}", stats_ini_path(&install).display());
    }
    let mut tracker = RlTracker::new();
    loop {
        match std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], ini.port()).into(), Duration::from_secs(1)) {
            Ok(stream) => {
                tracker.new_recording();
                if follow_rl(stream, &mut tracker, clock, &tx).is_err() {
                    return;
                }
            }
            // Menus or free play: the game serves nothing.
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// Reads one connection until the game closes it. `Err` when the receiver is gone.
fn follow_rl(mut stream: std::net::TcpStream, tracker: &mut RlTracker, clock: Clock, tx: &Sender<Vec<GameEvent>>) -> Result<(), ()> {
    use std::io::Write;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut framer = JsonStream::default();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut got_any, mut tried_ws) = (false, false);
    let opened = Instant::now();
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => 0,
            Err(_) => return Ok(()),
        };
        // Some builds speak WebSocket: ask for an upgrade once if nothing arrives (as Recap does).
        if n == 0 && !got_any && !tried_ws && opened.elapsed() > Duration::from_secs(3) {
            tried_ws = true;
            let req = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: cmVjYXAtcm9ja2V0LWxlYWd1ZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
            let _ = stream.write_all(req.as_bytes());
        }
        got_any |= n > 0;
        for msg in framer.push(&buf[..n]) {
            let now = clock.elapsed_ms();
            let events = tracker.ingest(&msg, now, Some(now));
            if !events.is_empty() {
                tx.send(events).map_err(|_| ())?;
            }
        }
    }
}

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        if let Ok(p) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Valve\Steam")
            .and_then(|k| k.get_value::<String, _>("SteamPath"))
        {
            roots.push(PathBuf::from(p.replace('/', "\\")));
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }
    roots.dedup();
    roots.into_iter().filter(|p: &PathBuf| Path::exists(p)).collect()
}
