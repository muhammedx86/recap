//! In-game notifications: a small transparent, always-on-top, click-through window that shows
//! short toasts (recording started/saved, bookmark, clips ready, match skipped).
//!
//! The window must never take focus from the game. It is created on the first toast, shown at
//! creation without activation (`focused(false)` + `focusable(false)`), and from then on only
//! moved on and off screen (moves never activate a window); `show()` is never called again.
//! Exclusive-fullscreen games draw over every window, so toasts show only in borderless or
//! windowed games.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::state::state;

pub const LABEL: &str = "overlay";
/// Event the overlay page listens to.
pub const EVENT: &str = "overlay-toast";
const WIDTH: f64 = 340.0;
const HEIGHT: f64 = 220.0;
/// Margin from the screen edge, logical px.
const MARGIN: f64 = 18.0;
/// How long the window stays on screen after the last toast (toasts fade out before this).
const LINGER: Duration = Duration::from_millis(5_200);
/// Where the window waits between toasts.
const PARKED: PhysicalPosition<i32> = PhysicalPosition { x: -32_000, y: -32_000 };

static LAST_TOAST: AtomicU64 = AtomicU64::new(0);
/// Only one thread may create the window.
static CREATE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToastKind {
    Recording,
    Saved,
    Bookmark,
    Clips,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// "top-right" | "top-left" | "bottom-right" | "bottom-left" (the page stacks toasts from it).
    pub corner: String,
}

/// Show a toast over the game (respects the notification settings).
pub fn notify(app: &AppHandle, kind: ToastKind, title: impl Into<String>, detail: Option<String>) {
    let (enabled, corner) = {
        let st = state(app);
        let s = st.settings.read().unwrap();
        let on = s.notifications && (kind != ToastKind::Bookmark || s.notify_on_bookmark);
        (on, s.notification_corner.clone())
    };
    if !enabled {
        return;
    }
    // The app shows its own toasts when it is in front.
    if app.get_webview_window("main").is_some_and(|w| w.is_focused().unwrap_or(false)) {
        return;
    }
    let toast = Toast {
        kind,
        title: title.into(),
        detail,
        corner: corner.clone(),
    };
    let app = app.clone();
    // Window work happens off the caller's thread (callers may hold locks).
    std::thread::spawn(move || {
        if let Err(e) = show(&app, &toast) {
            log::warn!("notification overlay: {e:#}");
        }
    });
}

fn show(app: &AppHandle, toast: &Toast) -> anyhow::Result<()> {
    let target = corner_position(app, &toast.corner);
    let creating = CREATE.lock().unwrap_or_else(|e| e.into_inner());
    let window = match app.get_webview_window(LABEL) {
        Some(w) => {
            if let Some(p) = target {
                w.set_position(p)?;
            }
            w
        }
        None => {
            let mut b = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
                .title("Recap notifications")
                .inner_size(WIDTH, HEIGHT)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .resizable(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focusable(false)
                .focused(false)
                .visible(true);
            if let Some(p) = target {
                let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
                b = b.position(p.x as f64 / scale, p.y as f64 / scale);
            }
            let w = b.build()?;
            w.set_ignore_cursor_events(true)?;
            // A brand-new page needs a moment before it listens for events.
            std::thread::sleep(Duration::from_millis(600));
            w
        }
    };
    drop(creating);
    window.emit_to(LABEL, EVENT, toast)?;

    // Park off screen once no toast has arrived for a while.
    let stamp = LAST_TOAST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::sleep(LINGER);
    if LAST_TOAST.load(Ordering::SeqCst) == stamp {
        let _ = window.set_position(PARKED);
    }
    Ok(())
}

/// Top-left position of the overlay window for `corner` on the primary monitor's work area.
fn corner_position(app: &AppHandle, corner: &str) -> Option<PhysicalPosition<i32>> {
    let m = app.primary_monitor().ok().flatten()?;
    let scale = m.scale_factor();
    let area = m.work_area();
    let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
    let margin = (MARGIN * scale) as i32;
    let (ax, ay) = (area.position.x, area.position.y);
    let (aw, ah) = (area.size.width as i32, area.size.height as i32);
    let right = corner.ends_with("right");
    let bottom = corner.starts_with("bottom");
    Some(PhysicalPosition {
        x: if right { ax + aw - w - margin } else { ax + margin },
        y: if bottom { ay + ah - h - margin } else { ay + margin },
    })
}
