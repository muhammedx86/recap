//! Global hotkeys: bookmark and manual record toggle.

use anyhow::{Context, Result};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::{
    session,
    state::{refresh_status, state},
};

pub fn register(app: &AppHandle) -> Result<()> {
    let gs = app.global_shortcut();
    gs.unregister_all().ok();
    let (bookmark, manual) = {
        let s = state(app).settings.read().unwrap().clone();
        (s.bookmark_hotkey, s.manual_record_hotkey)
    };
    if !bookmark.trim().is_empty() {
        gs.on_shortcut(bookmark.as_str(), |app, _s, ev| {
            if ev.state == ShortcutState::Pressed {
                session::bookmark(app);
            }
        })
        .with_context(|| format!("bookmark hotkey {bookmark:?} could not be registered"))?;
    }
    if !manual.trim().is_empty() && !manual.eq_ignore_ascii_case(&bookmark) {
        gs.on_shortcut(manual.as_str(), |app, _s, ev| {
            if ev.state == ShortcutState::Pressed {
                toggle_manual(app);
            }
        })
        .with_context(|| format!("record hotkey {manual:?} could not be registered"))?;
    }
    Ok(())
}

fn toggle_manual(app: &AppHandle) {
    let current = state(app).session.lock().unwrap().as_ref().map(|s| s.game.is_none());
    match current {
        // A manual recording is running: stop it.
        Some(true) => {
            let _ = session::stop(app);
        }
        // A game recording is running: leave it alone.
        Some(false) => refresh_status(app, Some("A game is already being recorded".into())),
        None => {
            if let Err(e) = session::start(app, None, None) {
                refresh_status(app, Some(format!("{e:#}")));
            }
        }
    }
}
