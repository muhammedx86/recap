//! Recap desktop app: wires the core, the recorder sidecar and the UI together.

mod commands;
mod hotkeys;
mod monitor;
mod paths;
mod processing;
mod recorder;
mod selftest;
mod session;
mod sources;
mod state;
mod studio;

use std::{
    collections::HashSet,
    fs,
    sync::{Mutex, RwLock},
};

use recap_core::{ffmpeg::Ffmpeg, games::load_games, library::Library, settings::Settings};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, WindowEvent,
};

use crate::{
    paths::Paths,
    recorder::{Recorder, RecorderSignal},
    state::{refresh_status, AppState, RecState, RecorderStatus},
};

fn show_main(app: &AppHandle) {
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
    });

    // Recordings cut short by a crash still have a playable (hybrid MP4) file: process them.
    for m in interrupted {
        if std::path::Path::new(&m.path).exists() {
            processing::spawn(handle.clone(), m.id, true);
        }
    }

    if let Err(e) = hotkeys::register(&handle) {
        log::warn!("hotkeys: {e:#}");
    }

    // Tray icon: keeps Recap running (and recording) when the window is closed.
    let open = MenuItem::with_id(app, "open", "Open Recap", true, None::<&str>)?;
    let bookmark = MenuItem::with_id(app, "bookmark", "Add bookmark", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &bookmark, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Recap")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "bookmark" => session::bookmark(app),
            "quit" => {
                let _ = session::stop(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    monitor::spawn(handle.clone());
    let gsi_app = handle.clone();
    std::thread::spawn(move || sources::start_cs2_gsi(gsi_app));
    refresh_status(&handle, None);
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing the window hides it to the tray; recording continues.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::list_matches,
            commands::get_match,
            commands::update_match,
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
            commands::studio_delete_detector,
            commands::open_path,
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
