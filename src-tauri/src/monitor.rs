//! Watches running processes and starts/stops game recordings.
//!
//! Two modes per game:
//! * **Per match** when the game has a live match signal (`matchwatch`: VALORANT, CS2): start when
//!   a match begins, stop when it ends, for every match while the game stays open. A manual stop
//!   skips just that match.
//! * **Per session** otherwise: record while the game runs; processing splits the session into
//!   matches with the game's match start/end detectors. A manual stop sticks until the game
//!   restarts, unless the game has such detectors: then recording resumes in the background and
//!   the rest of the skipped match is dropped when splitting.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use recap_core::{events::kind, matchgate::GateAction};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::AppHandle;

use crate::{session, state::state};

pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("process-monitor".into())
        .spawn(move || {
            let mut sys = System::new();
            // Game pids that already had a whole-session recording (or a failed attempt). They
            // are not auto-recorded again until they exit, so a manual stop sticks.
            let mut handled: HashSet<u32> = HashSet::new();
            loop {
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
                );
                // exe name (lowercase) -> pids (several processes can share a name)
                let mut running: HashMap<String, Vec<u32>> = HashMap::new();
                for (pid, p) in sys.processes() {
                    running
                        .entry(p.name().to_string_lossy().to_lowercase())
                        .or_default()
                        .push(pid.as_u32());
                }
                let alive: HashSet<u32> = running.values().flatten().copied().collect();
                handled.retain(|pid| alive.contains(pid));
                state(&app).resume_pids.lock().unwrap().retain(|pid| alive.contains(pid));
                tick(&app, &running, &mut handled);
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn monitor");
}

/// The game has detectors that mark match boundaries (so a session can be split).
fn splits_matches(g: &recap_core::games::LoadedGame) -> bool {
    g.ready_detectors()
        .any(|d| d.def.event == kind::MATCH_START || d.def.event == kind::MATCH_END)
}

fn tick(app: &AppHandle, running: &HashMap<String, Vec<u32>>, handled: &mut HashSet<u32>) {
    crate::selftest::poll_stop_request(app);
    let st = state(app);
    let now = crate::state::now_unix_ms();
    let active = st
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| (s.game.as_ref().map(|g| g.def.id.clone()), s.pid, s.stopping));
    match active {
        Some((Some(game_id), Some(pid), false)) => {
            if !running.values().flatten().any(|&p| p == pid) {
                log::info!("game process {pid} exited; stopping recording");
                let _ = session::stop(app);
                return;
            }
            // Per-match games: stop when the match is over.
            let action = st
                .gates
                .lock()
                .unwrap()
                .get(&game_id)
                .filter(|e| e.fresh())
                .map(|e| e.gate.decide(true, now));
            if action == Some(GateAction::Stop) {
                log::info!("{game_id}: match over; stopping recording");
                let _ = session::stop(app);
            }
        }
        Some(_) => {}
        None => {
            let settings = st.settings.read().unwrap().clone();
            let games = st.games.read().unwrap().clone();
            for g in games.iter().filter(|g| settings.game_enabled(&g.def.id)) {
                let pids: Vec<(u32, String)> = g
                    .def
                    .processes
                    .iter()
                    .flat_map(|p| {
                        running
                            .get(&p.to_lowercase())
                            .into_iter()
                            .flatten()
                            .map(move |pid| (*pid, p.clone()))
                    })
                    .collect();
                let Some((pid, exe)) = pids.first().cloned() else { continue };

                let gate = st
                    .gates
                    .lock()
                    .unwrap()
                    .get(&g.def.id)
                    .filter(|e| e.fresh())
                    .map(|e| e.gate.decide(false, now));
                if let Some(action) = gate {
                    // Per match: wait for the match to begin.
                    if action == GateAction::Start {
                        log::info!("{}: match started (pid {pid}); recording", g.def.name);
                        if let Err(e) = session::start(app, Some(g.clone()), Some((pid, exe))) {
                            log::error!("could not start recording for {}: {e:#}", g.def.name);
                        }
                        break;
                    }
                    continue;
                }

                // Per session.
                let resume = st.resume_pids.lock().unwrap().remove(&pid);
                if handled.contains(&pid) && !resume {
                    continue;
                }
                handled.insert(pid);
                log::info!(
                    "{} detected (pid {pid}); {}",
                    g.def.name,
                    if resume {
                        "resuming after a skipped match"
                    } else {
                        "starting recording"
                    }
                );
                match session::start(app, Some(g.clone()), Some((pid, exe))) {
                    Ok(()) if resume => session::mark_resumed(app),
                    Ok(()) => {}
                    Err(e) => log::error!("could not start recording for {}: {e:#}", g.def.name),
                }
                break;
            }
        }
    }
}

/// The player stopped the recording by hand: skip the rest of this match only.
pub fn on_manual_stop(app: &AppHandle) {
    let st = state(app);
    let (game, pid) = match st.session.lock().unwrap().as_ref() {
        Some(s) => (s.game.clone(), s.pid),
        None => return,
    };
    let (Some(g), Some(pid)) = (game, pid) else { return };
    if let Some(e) = st.gates.lock().unwrap().get_mut(&g.def.id).filter(|e| e.fresh()) {
        e.gate.skip_current();
        log::info!("{}: skipping the rest of this match", g.def.name);
        return;
    }
    if splits_matches(&g) {
        st.resume_pids.lock().unwrap().insert(pid);
        log::info!("{}: skipping this match; recording resumes for the next one", g.def.name);
    }
}
