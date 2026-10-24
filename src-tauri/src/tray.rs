//! Tray icon: keeps Recap running (and recording) when the window is closed.
//!
//! Menu: Open Recap · Start/Stop recording · Add bookmark · Open recordings folder · Quit.
//! A left click opens the window.

use std::sync::OnceLock;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Wry,
};

use crate::{
    session,
    state::{refresh_status, state, RecState, RecorderStatus},
};

static RECORD_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();
static BOOKMARK_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Recap", true, None::<&str>)?;
    let record = MenuItem::with_id(app, "record", "Start recording", true, None::<&str>)?;
    let bookmark = MenuItem::with_id(app, "bookmark", "Add bookmark", false, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open recordings folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Recap", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep, &record, &bookmark, &folder, &sep2, &quit])?;
    let _ = RECORD_ITEM.set(record);
    let _ = BOOKMARK_ITEM.set(bookmark);
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Recap")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => crate::show_main(app),
            "record" => toggle_recording(app),
            "bookmark" => session::bookmark(app),
            "folder" => {
                let dir = state(app).recordings_dir();
                let _ = std::fs::create_dir_all(&dir);
                if let Err(e) = tauri_plugin_opener::open_path(&dir, None::<&str>) {
                    log::warn!("could not open the recordings folder: {e}");
                }
            }
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                crate::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Start a manual recording, or stop the current one (in a game: skips the rest of the match).
fn toggle_recording(app: &AppHandle) {
    let recording = state(app).session.lock().unwrap().is_some();
    if recording {
        crate::monitor::on_manual_stop(app);
        let _ = session::stop(app);
    } else if let Err(e) = session::start(app, None, None) {
        refresh_status(app, Some(format!("{e:#}")));
    }
}

/// Keep the tray menu in step with the recorder (called on every status change).
pub fn sync(status: &RecorderStatus) {
    let recording = matches!(status.state, RecState::Recording | RecState::Starting);
    if let Some(item) = RECORD_ITEM.get() {
        let _ = item.set_text(if recording { "Stop recording" } else { "Start recording" });
        let _ = item.set_enabled(!matches!(status.state, RecState::Stopping | RecState::Unavailable));
    }
    if let Some(item) = BOOKMARK_ITEM.get() {
        let _ = item.set_enabled(status.state == RecState::Recording);
    }
}
