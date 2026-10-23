//! Watches running processes and starts/stops game recordings.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::AppHandle;

use crate::{session, state::state};

pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("process-monitor".into())
        .spawn(move || {
            let mut sys = System::new();
            // Game pids that already had a recording (or a failed attempt). They are not
            // auto-recorded again until they exit, so a manual stop sticks.
            let mut handled: HashSet<u32> = HashSet::new();
            loop {
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
                );
                // exe name (lowercase) -> pid
                let running: HashMap<String, u32> = sys
                    .processes()
                    .iter()
                    .map(|(pid, p)| (p.name().to_string_lossy().to_lowercase(), pid.as_u32()))
                    .collect();
                handled.retain(|pid| running.values().any(|p| p == pid));
                tick(&app, &running, &mut handled);
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn monitor");
}

fn tick(app: &AppHandle, running: &HashMap<String, u32>, handled: &mut HashSet<u32>) {
    let st = state(app);
    // Is the current game session's process gone?
    let active = st.session.lock().unwrap().as_ref().map(|s| (s.game.is_some(), s.pid, s.stopping));
    match active {
        Some((true, Some(pid), false)) => {
            if !running.values().any(|&p| p == pid) {
                log::info!("game process {pid} exited; stopping recording");
                let _ = session::stop(app);
            }
        }
        Some(_) => {}
        None => {
            let settings = st.settings.read().unwrap().clone();
            let games = st.games.read().unwrap().clone();
            for g in games.iter().filter(|g| settings.game_enabled(&g.def.id)) {
                let hit = g.def.processes.iter().find_map(|p| {
                    running
                        .get(&p.to_lowercase())
                        .filter(|pid| !handled.contains(pid))
                        .map(|pid| (*pid, p.clone()))
                });
                if let Some((pid, exe)) = hit {
                    handled.insert(pid);
                    log::info!("{} detected (pid {pid}); starting recording", g.def.name);
                    if let Err(e) = session::start(app, Some(g.clone()), Some((pid, exe))) {
                        log::error!("could not start recording for {}: {e:#}", g.def.name);
                    }
                    break;
                }
            }
        }
    }
}
