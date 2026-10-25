//! Recap desktop app: wires the core, the recorder sidecar and the UI together.

mod arming;
mod commands;
mod hotkeys;
mod matchwatch;
mod monitor;
mod ocr;
mod overlay;
mod paths;
mod processing;
mod recorder;
mod selftest;
mod session;
mod sources;
mod startup;
mod state;
mod studio;
mod tray;

use std::{
    collections::HashSet,
    fs,
    sync::{Mutex, RwLock},
};

use recap_core::{ffmpeg::Ffmpeg, games::load_games, library::Library, settings::Settings};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

use crate::{
    paths::Paths,
    recorder::{Recorder, RecorderSignal},
    state::{refresh_status, AppState, RecState, RecorderStatus},
};

/// Stop recording and exit (tray "Quit", or closing the window when "close to tray" is off).
pub(crate) fn quit(app: &AppHandle) {
    let _ = session::stop(app);
    app.exit(0);
}

/// Tell the UI the window was hidden or minimized, so it stops playing media.
fn window_hidden(window: &tauri::Window, reason: &str) {
    let _ = window.emit("window-hidden", serde_json::json!({ "reason": reason }));
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn gsi_token(paths: &Paths) -> String {
    let f = paths.data_dir.join("gsi-token.txt");
    if let Ok(t) = fs::read_to_string(&f) {
        if t.trim().len() >= 16 {
            return t.trim().to_string();
        }
    }
    let t = uuid::Uuid::new_v4().simple().to_string();
    let _ = fs::write(&f, &t);
    t
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let paths = Paths::resolve(&handle)?;
    fs::create_dir_all(&paths.data_dir)?;
    fs::create_dir_all(&paths.user_games)?;
    fs::create_dir_all(&paths.thumbnails)?;
    log::info!("data dir {}", paths.data_dir.display());
    log::info!("recorder {}", paths.recorder_exe.display());
    log::info!("bundled games {}", paths.bundled_games.display());

    let settings = Settings::load(&paths.settings_file);
    let library = Library::open(&paths.library_db)?;
    let interrupted = library.mark_interrupted_recordings()?;
    let ff = Ffmpeg::locate(&paths.ffmpeg_dirs);
    let games = load_games(&paths.game_roots());

    let rec_app = handle.clone();
    let recorder = Recorder::new(
        paths.recorder_exe.clone(),
        paths.data_dir.join("recorder.log"),
        move |sig| match sig {
            RecorderSignal::Event(ev) => session::on_recorder_event(&rec_app, ev),
            RecorderSignal::Exited => session::on_recorder_exited(&rec_app),
        },
    );
    let available = recorder.available();
    let token = gsi_token(&paths);

    app.manage(AppState {
        paths,
        settings: RwLock::new(settings),
        library: Mutex::new(library),
        games: RwLock::new(games),
        ff,
        recorder,
        session: Mutex::new(None),
        status: RwLock::new(RecorderStatus {
            state: if available { RecState::Idle } else { RecState::Unavailable },
            ..RecorderStatus::idle(None)
        }),
        processing: Mutex::new(HashSet::new()),
        gsi_token: token,
        gates: Default::default(),
        resume_pids: Default::default(),
        arming: Default::default(),
    });

    // A crash mid-match leaves the recording in the pre-roll buffer: move it where it belongs.
    {
        let st = state::state(&handle);
        arming::recover_buffer(&st);
        let gpu = st.settings.read().unwrap().gpu_preference;
        arming::gpu_preference::apply(&st.paths.recorder_exe, gpu);
    }
    // Recordings cut short by a crash still have a playable (hybrid MP4) file: process them.
    for m in interrupted {
        if std::path::Path::new(&m.path).exists() {
            processing::spawn(handle.clone(), m.id, true);
        }
    }

    // The main window starts hidden: show it unless Windows launched us at sign-in and the
    // player wants Recap to stay in the tray then.
    let stay_in_tray = startup::launched_minimized() && state::state(&handle).settings.read().unwrap().start_minimized;
    if !stay_in_tray {
        show_main(&handle);
    }
    let autostart = state::state(&handle).settings.read().unwrap().launch_at_startup;
    startup::apply(&handle, autostart);

    if let Err(e) = hotkeys::register(&handle) {
        log::warn!("hotkeys: {e:#}");
    }

    tray::build(app)?;

    monitor::spawn(handle.clone());
    matchwatch::spawn_riot_watcher(handle.clone());
    matchwatch::spawn_marvel_watcher(handle.clone());
    let gsi_app = handle.clone();
    std::thread::spawn(move || sources::start_cs2_gsi(gsi_app));
    refresh_status(&handle, None);
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        // Launching Recap again brings it to the front (but not the sign-in autostart entry).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !args.iter().any(|a| a == startup::MINIMIZED_ARG) {
                show_main(app);
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                    file_name: Some("recap".into()),
                }))
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout))
                .targets(std::env::var_os("RECAP_DATA_DIR").filter(|v| !v.is_empty()).map(|d| {
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                        path: std::path::PathBuf::from(d),
                        file_name: Some("recap".into()),
                    })
                }))
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![startup::MINIMIZED_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // Remember the window's size and position (not its visibility: hidden-to-tray must not
        // make the next launch start hidden).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .with_denylist(&["overlay"])
                .build(),
        )
        .setup(setup)
        .on_window_event(|window, event| match event {
            // Only the main window: the notification overlay is never closed or minimized by hand.
            _ if window.label() != "main" => {}
            // Closing the window hides it to the tray (recording continues), unless the player
            // chose "Quit when the window is closed".
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|st| st.settings.read().unwrap().close_to_tray)
                    .unwrap_or(true);
                if to_tray {
                    window_hidden(window, "closed");
                    let _ = window.hide();
                } else {
                    quit(window.app_handle());
                }
            }
            WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => window_hidden(window, "minimized"),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::list_matches,
            commands::get_match,
            commands::update_match,
            commands::update_match_detail,
            commands::delete_match,
            commands::add_event,
            commands::delete_event,
            commands::rescan_match,
            commands::list_clips,
            commands::create_clip,
            commands::create_montage,
            commands::update_clip,
            commands::delete_clip,
            commands::list_games,
            commands::set_game_enabled,
            commands::save_game,
            commands::list_event_types,
            commands::save_event_type,
            commands::delete_event_type,
            commands::get_settings,
            commands::update_settings,
            commands::list_encoders,
            commands::list_audio_inputs,
            commands::start_manual_recording,
            commands::stop_recording,
            commands::add_bookmark,
            commands::get_storage,
            commands::studio_load_frame,
            commands::studio_save_detector,
            commands::studio_test_detector,
            commands::studio_video_info,
            commands::studio_cancel,
            commands::studio_delete_detector,
            commands::studio_save_reader,
            commands::studio_read_frame,
            commands::studio_test_reader,
            commands::studio_delete_reader,
            commands::open_path,
            commands::set_fullscreen,
            commands::refresh,
            commands::selftest_enabled,
            commands::selftest_tour_step,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Recap");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Stop cleanly and give the recorder time to finalise the file.
            let _ = session::stop(app);
            if let Some(st) = app.try_state::<AppState>() {
                for _ in 0..150 {
                    if st.session.lock().unwrap().is_none() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                st.recorder.shutdown();
            }
        }
    });
}
