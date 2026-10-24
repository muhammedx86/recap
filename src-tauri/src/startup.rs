//! Launch with Windows (opt-in) and start in the tray when launched that way.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// Argument the autostart entry passes, so a login launch can stay in the tray.
pub const MINIMIZED_ARG: &str = "--minimized";

/// Launched by the autostart entry (at sign-in).
pub fn launched_minimized() -> bool {
    std::env::args().any(|a| a == MINIMIZED_ARG)
}

/// Make the system autostart entry match the setting.
pub fn apply(app: &AppHandle, launch_at_startup: bool) {
    let al = app.autolaunch();
    let enabled = al.is_enabled().unwrap_or(false);
    let res = match (launch_at_startup, enabled) {
        (true, false) => al.enable(),
        (false, true) => al.disable(),
        _ => Ok(()),
    };
    if let Err(e) = res {
        log::warn!("launch with Windows: {e}");
    }
}
