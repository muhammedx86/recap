//! Recording session lifecycle: start, bookmarks, stop, and recorder events.

use std::{
    fs,
    path::PathBuf,
    sync::{atomic::AtomicBool, atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use recap_core::{
    cs2::Cs2Tracker,
    events::{kind, EventSource, GameEvent},
    games::{ApiSource, LoadedGame},
    library::{MatchRecord, MatchStatus},
    protocol::{RecorderCommand, RecorderEvent},
};
use tauri::{AppHandle, Emitter};

use crate::{
    arming,
    paths::sanitize,
    processing, sources,
    state::{add_live_events, now_unix_ms, refresh_status, state, Session},
};

/// Start recording. `game` None = manual recording of the primary monitor.
/// `process` is the (pid, exe name) that triggered the recording, if any.
pub fn start(app: &AppHandle, game: Option<LoadedGame>, process: Option<(u32, String)>) -> Result<()> {
    let pid = process.as_ref().map(|p| p.0);
    let st = state(app);
    if !st.recorder.available() {
        bail!("The recorder is not installed (missing {}).", st.paths.recorder_exe.display());
    }
    if st.session.lock().unwrap().is_some() {
        bail!("Already recording");
    }

    let settings = st.settings.read().unwrap().clone();
    let match_id = uuid::Uuid::new_v4().to_string();
    let game_name = game
        .as_ref()
        .map(|g| g.def.name.clone())
        .unwrap_or_else(|| "Manual recording".into());
    let game_id = game.as_ref().map(|g| g.def.id.clone()).unwrap_or_else(|| "manual".into());
    let dir = st.recordings_dir().join(sanitize(&game_name));
    fs::create_dir_all(&dir)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let path: PathBuf = dir.join(format!("{stamp}.mp4"));

    // The same setup as the arm (see `arming`), so the recorder reuses the attached capture
    // and its pre-roll buffer.
    let cmd = RecorderCommand::Start {
        id: match_id.clone(),
        path: path.to_string_lossy().into_owned(),
        capture: arming::capture_target(game.as_ref(), process.as_ref()),
        video: arming::video_config(&settings),
        audio: arming::audio_config(&settings),
        encoder_memory: arming::load_memory(&st),
    };

    let record = MatchRecord {
        id: match_id.clone(),
        game_id,
        game_name,
        path: path.to_string_lossy().into_owned(),
        started_unix_ms: now_unix_ms(),
        duration_ms: 0,
        size_bytes: 0,
        status: MatchStatus::Recording,
        title: None,
        favorite: false,
        thumbnail: None,
        event_counts: Default::default(),
        error: None,
        details: Default::default(),
    };
    // Never hold the session lock while taking the library lock (lock order: library, then session).
    st.library.lock().unwrap().insert_match(&record)?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let api = game.as_ref().and_then(|g| g.def.api);
    let mut session_guard = st.session.lock().unwrap();
    if session_guard.is_some() {
        // Lost a race with another start: discard our row.
        drop(session_guard);
        let _ = st.library.lock().unwrap().delete_match(&match_id);
        bail!("Already recording");
    }
    *session_guard = Some(Session {
        match_id: match_id.clone(),
        game,
        pid,
        path,
        requested: Instant::now(),
        started: None,
        started_unix_ms: record.started_unix_ms,
        stop_flag: stop_flag.clone(),
        bookmarks: 0,
        stopping: false,
        cs2: (api == Some(ApiSource::Cs2Gsi)).then(|| Cs2Tracker::new(&st.gsi_token)),
    });
    drop(session_guard);

    if let Err(e) = st.recorder.send(&cmd) {
        fail_start(app, &match_id, &format!("{e:#}"));
        bail!("{e:#}");
    }
    if api == Some(ApiSource::LeagueLiveClient) {
        sources::spawn_league_poller(app.clone(), match_id.clone(), stop_flag);
    }
    let _ = app.emit("library-changed", serde_json::json!({ "matchId": match_id }));
    refresh_status(app, None);
    Ok(())
}

fn fail_start(app: &AppHandle, match_id: &str, message: &str) {
    let st = state(app);
    {
        let mut s = st.session.lock().unwrap();
        if s.as_ref().is_some_and(|s| s.match_id == match_id) {
            if let Some(s) = s.take() {
                s.stop_flag.store(true, Ordering::Relaxed);
            }
        }
    }
    let lib = st.library.lock().unwrap();
    if let Ok(Some(mut m)) = lib.get_match(match_id) {
        m.status = MatchStatus::Error;
        m.error = Some(message.to_string());
        let _ = lib.update_match(&m);
    }
    drop(lib);
    let _ = app.emit("library-changed", serde_json::json!({ "matchId": match_id }));
    refresh_status(app, Some(format!("Recording failed: {message}")));
}

/// Ask the recorder to stop. Finalisation happens when `Stopped` arrives (or after a timeout).
pub fn stop(app: &AppHandle) -> Result<()> {
    let st = state(app);
    let match_id = {
        let mut guard = st.session.lock().unwrap();
        let Some(s) = guard.as_mut() else { return Ok(()) };
        if s.stopping {
            return Ok(());
        }
        s.stopping = true;
        s.stop_flag.store(true, Ordering::Relaxed);
        s.match_id.clone()
    };
    refresh_status(app, None);
    if let Err(e) = st.recorder.send(&RecorderCommand::Stop { id: match_id.clone() }) {
        log::error!("stop command failed: {e:#}");
        finalize(app, &match_id, None);
        return Ok(());
    }
    // Watchdog: if the recorder never confirms, finalise anyway so the UI does not hang.
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        let still = state(&app2)
            .session
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.match_id == match_id);
        if still {
            log::warn!("recorder did not confirm stop; restarting it and finalising {match_id}");
            // Kill the stuck recorder so the next recording starts on a fresh process.
            state(&app2).recorder.kill();
            finalize(&app2, &match_id, None);
        }
    });
    Ok(())
}

/// Hidden marker on a recording that resumed in the middle of a match the player skipped.
pub const RESUMED_KIND: &str = "_resumed";

/// Mark the current recording as resumed after a skipped match (see `monitor`).
pub fn mark_resumed(app: &AppHandle) {
    let id = state(app).session.lock().unwrap().as_ref().map(|s| s.match_id.clone());
    if let Some(id) = id {
        add_live_events(app, &id, vec![GameEvent::new(RESUMED_KIND, 0, EventSource::Manual)]);
    }
}

/// Drop a bookmark on the current recording.
pub fn bookmark(app: &AppHandle) {
    let st = state(app);
    let (match_id, t) = {
        let mut guard = st.session.lock().unwrap();
        let Some(s) = guard.as_mut() else { return };
        if s.started.is_none() || s.stopping {
            return;
        }
        s.bookmarks += 1;
        (s.match_id.clone(), s.elapsed_ms())
    };
    add_live_events(app, &match_id, vec![GameEvent::new(kind::BOOKMARK, t, EventSource::Manual)]);
    crate::overlay::notify(
        app,
        crate::overlay::ToastKind::Bookmark,
        "Bookmark added",
        Some(format!("{}:{:02}", t / 60_000, (t / 1000) % 60)),
    );
    let _ = app.emit("bookmark-added", serde_json::json!({ "matchId": match_id, "tMs": t }));
    refresh_status(app, None);
}

/// Remove the session and hand the file to the processing pipeline.
fn finalize(app: &AppHandle, match_id: &str, duration_ms: Option<u64>) {
    let st = state(app);
    let session = {
        let mut guard = st.session.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.match_id == match_id) {
            guard.take()
        } else {
            None
        }
    };
    let Some(session) = session else { return };
    session.stop_flag.store(true, Ordering::Relaxed);
    let elapsed = session.started.map(|s| s.elapsed().as_millis() as u64);
    {
        let lib = st.library.lock().unwrap();
        if let Ok(Some(mut m)) = lib.get_match(match_id) {
            m.duration_ms = duration_ms.or(elapsed).unwrap_or(0);
            m.size_bytes = fs::metadata(&session.path).map(|m| m.len()).unwrap_or(0);
            if session.path.exists() && m.size_bytes > 0 {
                m.status = MatchStatus::Processing;
            } else {
                m.status = MatchStatus::Error;
                m.error = Some("No video was written".into());
            }
            let _ = lib.update_match(&m);
        }
    }
    refresh_status(app, None);
    let _ = app.emit("library-changed", serde_json::json!({ "matchId": match_id }));
    if session.path.exists() {
        let len = duration_ms.or(elapsed).unwrap_or(0) / 1000;
        crate::overlay::notify(
            app,
            crate::overlay::ToastKind::Saved,
            "Recording saved",
            Some(format!("{} · {}:{:02}", session.game_name(), len / 60, len % 60)),
        );
        processing::spawn(app.clone(), match_id.to_string(), false);
    }
}

/// The recorder process exited (crash or kill). Keep whatever was written.
pub fn on_recorder_exited(app: &AppHandle) {
    let st = state(app);
    // A recording in the pre-roll buffer is moved to its match path first.
    arming::on_recorder_exited(&st);
    let current = st
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| (s.match_id.clone(), s.started.is_some()));
    match current {
        Some((mid, true)) => {
            finalize(app, &mid, None);
            refresh_status(app, Some("The recorder stopped unexpectedly; the recording was saved".into()));
        }
        Some((mid, false)) => fail_start(app, &mid, "The recorder stopped unexpectedly"),
        None => refresh_status(app, None),
    }
}

/// Called on the recorder reader thread.
pub fn on_recorder_event(app: &AppHandle, ev: RecorderEvent) {
    log::debug!("recorder: {ev:?}");
    let st = state(app);
    match ev {
        RecorderEvent::Ready { obs_version } => {
            log::info!("recorder ready (OBS {obs_version})");
            refresh_status(app, None);
        }
        RecorderEvent::Started {
            id,
            started_unix_ms,
            encoder,
            capture,
            preroll_ms,
            encoder_fallback,
            encoder_memory,
            ..
        } => {
            if let Some(m) = &encoder_memory {
                arming::save_memory(&st, m);
            }
            {
                let mut guard = st.session.lock().unwrap();
                if let Some(s) = guard.as_mut().filter(|s| s.match_id == id) {
                    // The file begins `preroll_ms` before now: event times count from there.
                    let now = Instant::now();
                    s.started = Some(now.checked_sub(Duration::from_millis(preroll_ms)).unwrap_or(now));
                    s.started_unix_ms = started_unix_ms;
                }
            }
            if preroll_ms > 0 {
                log::info!("{id}: recording kept {preroll_ms} ms from before the start");
            }
            if let Some(f) = &encoder_fallback {
                log::warn!("{id}: {f}");
            }
            {
                let lib = st.library.lock().unwrap();
                if let Ok(Some(mut m)) = lib.get_match(&id) {
                    m.started_unix_ms = started_unix_ms;
                    let _ = lib.update_match(&m);
                }
            }
            let game = st
                .session
                .lock()
                .unwrap()
                .as_ref()
                .filter(|s| s.match_id == id)
                .map(|s| s.game_name());
            {
                let mut status = st.status.write().unwrap();
                status.encoder = Some(encoder);
                status.capture = Some(capture);
                status.match_id = Some(id);
            }
            refresh_status(app, encoder_fallback);
            if let Some(name) = game {
                crate::overlay::notify(app, crate::overlay::ToastKind::Recording, "Recording", Some(name));
            }
        }
        RecorderEvent::CaptureChanged { id, capture, reason } => {
            {
                let mut status = st.status.write().unwrap();
                if status.match_id.as_deref() == Some(&id) {
                    status.capture = Some(capture);
                }
            }
            refresh_status(app, Some(reason));
        }
        RecorderEvent::Stopped {
            id, duration_ms, health, ..
        } => {
            if let Some(h) = health {
                arming::record_health(app, &id, h);
            }
            finalize(app, &id, Some(duration_ms))
        }
        RecorderEvent::Armed {
            capture,
            encoder,
            buffering,
            encoder_fallback,
            encoder_memory,
        } => {
            log::info!("recorder armed: {capture:?} capture, encoder {encoder:?}, pre-roll buffer {buffering}");
            if let Some(m) = &encoder_memory {
                arming::save_memory(&st, m);
            }
            st.arming.lock().unwrap().last_armed = Some(serde_json::json!({
                "capture": capture, "encoder": encoder, "buffering": buffering, "encoderFallback": encoder_fallback,
            }));
            if let Some(f) = encoder_fallback {
                log::warn!("recorder: {f}");
                refresh_status(app, Some(f));
            }
        }
        RecorderEvent::Disarmed => log::info!("recorder disarmed"),
        RecorderEvent::Warning { id, code, message } => {
            log::warn!("recorder warning {code} ({id:?}): {message}");
            refresh_status(app, Some(message));
        }
        RecorderEvent::Error { id, message } => {
            log::error!("recorder error ({id:?}): {message}");
            let current = st
                .session
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| (s.match_id.clone(), s.started.is_some()));
            match (current, id) {
                // Only an error about *this* recording ends it; id-less errors (bad command,
                // encoder listing) are informational.
                (Some((mid, started)), Some(eid)) if eid == mid => {
                    if started {
                        finalize(app, &mid, None);
                        refresh_status(app, Some(message));
                    } else {
                        fail_start(app, &mid, &message);
                    }
                }
                _ => refresh_status(app, Some(message)),
            }
        }
        RecorderEvent::Encoders { .. } => {}
    }
}
