//! `recap-recorder`: the recording sidecar spawned by the Recap app.
//!
//! Protocol (see `recap_core::protocol`): one JSON command per line on stdin,
//! one JSON event per line on stdout, human-readable logs on stderr.

// Off Windows only the protocol plumbing (and its tests) is built.
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(any(windows, feature = "linux-typecheck"))]
mod obs_backend;
mod select;
mod session;

use std::{
    io::{BufRead, Write},
    sync::mpsc,
    thread,
};

use recap_core::protocol::RecorderEvent;
use session::{parse_line, Incoming};

/// Write one protocol event to stdout and flush.
fn emit(event: RecorderEvent) {
    let line = serde_json::to_string(&event).expect("events always serialize");
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Reads stdin on its own thread so long operations can still notice Stop/Shutdown.
fn spawn_stdin_reader() -> mpsc::Receiver<Incoming> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if let Some(msg) = parse_line(&line) {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Incoming::Eof);
        })
        .expect("spawning stdin reader");
    rx
}

/// OBS loads its data and plugins relative to the executable; also make the working
/// directory match so any relative path inside OBS resolves the same way.
fn enter_exe_dir() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        if let Err(e) = std::env::set_current_dir(&dir) {
            eprintln!("[recorder] could not enter {}: {e}", dir.display());
        }
    }
}

#[cfg(windows)]
fn main() {
    enter_exe_dir();
    let rx = spawn_stdin_reader();
    let (mut backend, obs_version) = match obs_backend::ObsBackend::new() {
        Ok(b) => b,
        Err(e) => {
            emit(RecorderEvent::Error {
                id: None,
                message: format!("{e:#}"),
            });
            std::process::exit(1);
        }
    };
    eprintln!("[recorder] OBS {obs_version} ready");
    emit(RecorderEvent::Ready { obs_version });
    session::run(&mut backend, rx, &mut emit);
    eprintln!("[recorder] exiting");
}

#[cfg(not(windows))]
fn main() {
    emit(RecorderEvent::Error {
        id: None,
        message: "recap-recorder only supports Windows".into(),
    });
    std::process::exit(1);
}
