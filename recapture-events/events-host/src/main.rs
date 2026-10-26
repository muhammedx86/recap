//! Recapture.Events: game event detection for Recapture, built on Recap's core (see NOTICE).
//!
//! `Recapture.Events.exe --game <game id or exe name> [--exe <full path>] [--root <app folder>] [--stdout]`
//!
//! Started by the Recapture service when a game starts, killed when it exits. Loads
//! `<root>\events\config.yaml` and the game data in `<root>\events\games`, runs the game's official
//! local source, and sends events and clip windows to `\\.\pipe\recapture-events` (`--stdout`
//! prints them instead, for testing). Exits when the pipe closes.
//!
//! `Recapture.Events.exe --game <id> --scan <clip> [--ffmpeg <dir>] [--root <app folder>]` scans a saved clip's
//! picture instead and prints what it finds (see `scan.rs`).

mod config;
mod planner;
mod protocol;
mod scan;
mod sources;

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use recap_core::games::{load_games, LoadedGame};

use config::Config;
use planner::Planner;
use protocol::{Message, PIPE_NAME};
use sources::Clock;

/// How often a quiet module writes, to notice the engine is gone.
const PING_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
struct Args {
    game: String,
    exe: Option<PathBuf>,
    root: Option<PathBuf>,
    stdout: bool,
    scan: Option<PathBuf>,
    ffmpeg: Vec<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--game" => a.game = it.next().context("--game needs a value")?,
            "--exe" => a.exe = it.next().map(PathBuf::from),
            "--root" => a.root = it.next().map(PathBuf::from),
            "--stdout" => a.stdout = true,
            "--scan" => a.scan = Some(it.next().context("--scan needs a clip")?.into()),
            "--ffmpeg" => a.ffmpeg.push(it.next().context("--ffmpeg needs a folder")?.into()),
            other => bail!("unknown argument {other}"),
        }
    }
    if a.game.is_empty() {
        bail!("usage: Recapture.Events --game <id or exe> [--exe <path>] [--root <dir>] [--stdout]");
    }
    Ok(a)
}

/// The game by id, or by one of its process names (any case, `.exe` optional).
fn find_game(games: Vec<LoadedGame>, key: &str) -> Option<LoadedGame> {
    let key = key.to_lowercase();
    let exe = if key.ends_with(".exe") { key.clone() } else { format!("{key}.exe") };
    games
        .into_iter()
        .find(|g| g.def.id.eq_ignore_ascii_case(&key) || g.def.processes.iter().any(|p| p.eq_ignore_ascii_case(&exe)))
}

/// Opens the engine's pipe, retrying for a while (the engine may still be starting).
fn open_pipe() -> Result<std::fs::File> {
    let mut last = None;
    for _ in 0..30 {
        match std::fs::OpenOptions::new().write(true).open(PIPE_NAME) {
            Ok(f) => return Ok(f),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(last.map(anyhow::Error::from).unwrap_or_else(|| anyhow::anyhow!("no pipe"))).context(PIPE_NAME)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Recapture.Events: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    init_log();
    let args = parse_args()?;
    let root = match args.root.clone() {
        Some(r) => r,
        None => std::env::current_exe()?.parent().context("exe folder")?.to_path_buf(),
    };
    let dir = root.join("events");
    let config = Config::load(&dir.join("config.yaml"))?;
    if !config.enabled {
        log::info!("game events are off in config.yaml");
        return Ok(());
    }
    let Some(game) = find_game(load_games(&[dir.join("games")]), &args.game) else {
        // Most games have no event data; the engine starts this for every game.
        log::info!("no game data for {}; nothing to do", args.game);
        return Ok(());
    };
    if let Some(clip) = &args.scan {
        return scan::run(&game, clip, &args.ffmpeg);
    }
    let Some(api) = game.def.api else {
        // Screen-only games get their events from saved clips (vision scan), not live.
        log::info!("{} has no live source; nothing to do", game.def.name);
        return Ok(());
    };

    let mut sink: Box<dyn Write> = if args.stdout { Box::new(std::io::stdout()) } else { Box::new(open_pipe()?) };
    let send = |sink: &mut Box<dyn Write>, msgs: &mut Vec<Message>| -> Result<()> {
        for m in msgs.drain(..) {
            sink.write_all(m.to_line().as_bytes())?;
        }
        sink.flush()?;
        Ok(())
    };
    let mut out = vec![Message::Hello { game: game.def.id.clone(), pid: std::process::id() }];
    send(&mut sink, &mut out).context("engine closed the pipe")?;

    let clock = Clock::new();
    let (tx, rx) = mpsc::channel();
    sources::start(api, clock, tx, args.exe.clone());
    log::info!("{}: forwarding {api:?} events", game.def.name);

    let mut planner = Planner::default();
    let mut last_sent = std::time::Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(events) => {
                // `_`-kinds are Recap's match-detail snapshots (map, champion...), not plays: not forwarded (OCR/details later).
                for mut ev in events.into_iter().filter(|e| !e.kind.starts_with('_')) {
                    ev.t_ms = clock.to_unix(ev.t_ms);
                    let action = config.resolve(&game.def, &ev.kind);
                    planner.push(ev, action, &mut out);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // The source thread ended (port taken, no install): nothing more will come.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                planner.flush(&mut out);
                send(&mut sink, &mut out)?;
                bail!("{api:?} source stopped");
            }
        }
        planner.tick(clock.now_unix(), &mut out);
        if out.is_empty() && last_sent.elapsed() >= PING_EVERY {
            out.push(Message::Ping);
        }
        if !out.is_empty() {
            last_sent = std::time::Instant::now();
        }
        if let Err(e) = send(&mut sink, &mut out) {
            log::info!("engine closed the pipe ({e}); exiting");
            return Ok(());
        }
    }
}

/// Logs to stderr (the service captures it) at info, or `RECAPTURE_EVENTS_LOG=debug`.
fn init_log() {
    struct Stderr(log::LevelFilter);
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= self.0
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    let level = match std::env::var("RECAPTURE_EVENTS_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    };
    if log::set_boxed_logger(Box::new(Stderr(level))).is_ok() {
        log::set_max_level(level);
    }
}
