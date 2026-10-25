//! Keeps the recorder ready while a game runs ("armed"): the capture, audio and encoder stay
//! attached between matches, and with a pre-roll the recorder buffers the last seconds so a
//! recording begins before the match start was noticed. Also: what the recorder learned about
//! the encoders (kept between runs), per-recording frame stats, and the hybrid-GPU preference.

use std::{
    fs,
    path::{Path, PathBuf},
};

use recap_core::{
    games::{CaptureMode, LoadedGame},
    prebuffer,
    protocol::{AudioConfig, CaptureTarget, EncoderMemory, RecorderCommand, RecordingHealth, VideoConfig},
    settings::Settings,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::state::{state, AppState};

/// Recorder readiness as the app knows it.
#[derive(Default)]
pub struct Arming {
    /// The last `Arm`/`Disarm` sent (JSON), so it is only re-sent when something changes.
    sent: Option<String>,
    /// The last `Armed` event (for the self-test report).
    pub last_armed: Option<Value>,
    /// Frame stats of recent recordings (match id, stats), newest last.
    pub health: Vec<(String, RecordingHealth)>,
}

pub fn video_config(s: &Settings) -> VideoConfig {
    VideoConfig {
        fps: s.fps,
        bitrate_kbps: s.bitrate_kbps,
        output_height: s.output_height,
        encoder: s.encoder.clone(),
        rate_control: s.rate_control,
        quality: s.quality,
    }
}

pub fn audio_config(s: &Settings) -> AudioConfig {
    AudioConfig {
        desktop: s.record_desktop_audio,
        microphone: s.record_microphone,
        microphone_device: s.microphone_device.clone(),
        separate_mic_track: s.separate_mic_track,
    }
}

/// What to capture for `game` (None = the primary monitor, a manual recording).
pub fn capture_target(game: Option<&LoadedGame>, process: Option<&(u32, String)>) -> CaptureTarget {
    match game {
        Some(g) => CaptureTarget {
            mode: g.def.capture,
            process: process.map(|p| p.1.clone()).or_else(|| g.def.processes.first().cloned()),
            pid: process.map(|p| p.0),
            monitor: None,
        },
        None => CaptureTarget {
            mode: CaptureMode::Monitor,
            process: None,
            pid: None,
            monitor: Some(0),
        },
    }
}

/// Folder for the pre-roll segments: inside the recordings folder, so finished recordings are
/// renamed, not copied.
pub fn buffer_dir(st: &AppState) -> PathBuf {
    st.recordings_dir().join(prebuffer::BUFFER_DIR_NAME)
}

fn memory_file(st: &AppState) -> PathBuf {
    st.paths.data_dir.join("encoder-memory.json")
}

pub fn load_memory(st: &AppState) -> Option<EncoderMemory> {
    fs::read_to_string(memory_file(st)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

pub fn save_memory(st: &AppState, m: &EncoderMemory) {
    if load_memory(st).as_ref() == Some(m) {
        return;
    }
    match serde_json::to_string_pretty(m) {
        Ok(t) => {
            if let Err(e) = fs::write(memory_file(st), t) {
                log::warn!("saving encoder memory: {e}");
            }
        }
        Err(e) => log::warn!("encoder memory: {e}"),
    }
}

/// Arm the recorder for `game` (its pid and exe name), or disarm it (None). Sends only changes.
pub fn sync(app: &AppHandle, game: Option<(&LoadedGame, u32, String)>) {
    let st = state(app);
    if !st.recorder.available() {
        return;
    }
    let cmd = match game {
        Some((g, pid, exe)) => {
            let settings = st.settings.read().unwrap().clone();
            let buffer = buffer_dir(&st);
            Some(RecorderCommand::Arm {
                capture: capture_target(Some(g), Some(&(pid, exe))),
                video: video_config(&settings),
                audio: audio_config(&settings),
                buffer_dir: (settings.preroll_secs > 0).then(|| buffer.to_string_lossy().into_owned()),
                preroll_secs: settings.preroll_secs,
                encoder_memory: None,
            })
        }
        None => None,
    };
    let key = match &cmd {
        Some(c) => serde_json::to_string(c).unwrap_or_default(),
        None => "disarm".to_string(),
    };
    {
        let mut a = st.arming.lock().unwrap();
        if a.sent.as_deref() == Some(key.as_str()) || (a.sent.is_none() && cmd.is_none()) {
            return;
        }
        a.sent = Some(key);
    }
    let cmd = match cmd {
        // The memory is added after the comparison (it changes as the recorder learns).
        Some(RecorderCommand::Arm {
            capture,
            video,
            audio,
            buffer_dir,
            preroll_secs,
            ..
        }) => {
            log::info!("arming the recorder for {:?} (pre-roll {preroll_secs} s)", capture.process);
            RecorderCommand::Arm {
                capture,
                video,
                audio,
                buffer_dir,
                preroll_secs,
                encoder_memory: load_memory(&st),
            }
        }
        _ => {
            log::info!("disarming the recorder");
            RecorderCommand::Disarm
        }
    };
    if let Err(e) = st.recorder.send(&cmd) {
        log::warn!("arming: {e:#}");
        st.arming.lock().unwrap().sent = None;
    }
}

/// The recorder process went away: it has to be armed again, and a recording it was writing
/// into the buffer is moved to where it belongs.
pub fn on_recorder_exited(st: &AppState) {
    st.arming.lock().unwrap().sent = None;
    for p in prebuffer::recover(&buffer_dir(st)) {
        log::info!("recovered {} from the pre-roll buffer", p.display());
    }
}

/// Move recordings a crash left in the buffer (call at startup, before the recorder runs).
pub fn recover_buffer(st: &AppState) {
    let dir = buffer_dir(st);
    for p in prebuffer::recover(&dir) {
        log::info!("recovered {} from the pre-roll buffer", p.display());
    }
    prebuffer::clean(&dir);
}

/// A recording finished with these frame stats.
pub fn record_health(app: &AppHandle, match_id: &str, h: RecordingHealth) {
    let st = state(app);
    let lost = h.lost_share().unwrap_or(0.0);
    log::info!(
        "{match_id}: {} frames, {} dropped, {} lagged ({:.1}% lost) with {}{}",
        h.total_frames,
        h.dropped_frames,
        h.lagged_frames,
        lost * 100.0,
        h.encoder,
        if h.preroll_ms > 0 {
            format!(", {} ms pre-roll", h.preroll_ms)
        } else {
            String::new()
        }
    );
    let _ = app.emit(
        "recording-health",
        serde_json::json!({ "matchId": match_id, "health": h, "lostShare": h.lost_share() }),
    );
    // Keep the essentials on the match so the library can badge it.
    let stored = recap_core::library::MatchHealth {
        dropped_frames: h.lost_share(),
        encoder: Some(encoder_label(&h.encoder)),
        ..Default::default()
    };
    if let Err(e) = st.library.lock().unwrap().set_health(match_id, &stored) {
        log::warn!("{match_id}: could not store recording health: {e:#}");
    }
    let mut a = st.arming.lock().unwrap();
    a.health.push((match_id.to_string(), h));
    let n = a.health.len();
    if n > 20 {
        a.health.drain(..n - 20);
    }
}

/// "NVENC", "Quick Sync", "AMF" or "x264 (CPU)" for an OBS encoder id.
fn encoder_label(id: &str) -> String {
    let l = id.to_ascii_lowercase();
    let name = if l.contains("nvenc") {
        "NVENC"
    } else if l.contains("qsv") {
        "Quick Sync"
    } else if l.contains("amf") {
        "AMF"
    } else if l.contains("x264") {
        "x264 (CPU)"
    } else {
        return id.to_string();
    };
    let codec = if l.contains("hevc") || l.contains("h265") {
        " HEVC"
    } else if l.contains("av1") {
        " AV1"
    } else {
        ""
    };
    format!("{name}{codec}")
}

/// Windows' per-app GPU choice (Settings > System > Display > Graphics), for the recorder only.
pub mod gpu_preference {
    use super::Path;

    /// The value Windows stores for "High performance".
    #[cfg_attr(not(windows), allow(dead_code))]
    pub const HIGH_PERFORMANCE: &str = "GpuPreference=2;";

    /// Set (or, when `on` is false, remove our own) high-performance preference for `exe`.
    /// Takes effect the next time the recorder starts.
    pub fn apply(exe: &Path, on: bool) {
        #[cfg(windows)]
        {
            use winreg::{enums::HKEY_CURRENT_USER, RegKey};
            // Windows keys the preference by the plain full path of the executable.
            let full = exe.to_string_lossy().to_string();
            let name = full.strip_prefix(r"\\?\").unwrap_or(&full).to_string();
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let path = r"Software\Microsoft\DirectX\UserGpuPreferences";
            if on {
                match hkcu.create_subkey(path) {
                    Ok((key, _)) => {
                        let current: Option<String> = key.get_value(&name).ok();
                        if current.as_deref() != Some(HIGH_PERFORMANCE) {
                            if let Err(e) = key.set_value(&name, &HIGH_PERFORMANCE) {
                                log::warn!("GPU preference: {e}");
                            } else {
                                log::info!("GPU preference: high performance for {name}");
                            }
                        }
                    }
                    Err(e) => log::warn!("GPU preference: {e}"),
                }
            } else if let Ok(key) = hkcu.open_subkey_with_flags(path, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE) {
                // Only undo what we set; a choice the player made in Windows stays.
                let current: Option<String> = key.get_value(&name).ok();
                if current.as_deref() == Some(HIGH_PERFORMANCE) {
                    let _ = key.delete_value(&name);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (exe, on);
    }
}

/// Self-test view of what the recorder reported.
pub fn report(st: &AppState) -> Value {
    let a = st.arming.lock().unwrap();
    serde_json::json!({
        "armed": a.last_armed,
        "recordings": a.health.iter().map(|(id, h)| serde_json::json!({ "matchId": id, "health": h })).collect::<Vec<_>>(),
        "bufferLeftovers": fs::read_dir(buffer_dir(st)).map(|d| d.flatten().filter(|e| prebuffer::is_segment(&e.path()) && prebuffer::marker_path(&e.path()).exists()).count()).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_arm_describe_the_same_setup() {
        let s = Settings::default();
        assert_eq!(video_config(&s).rate_control, recap_core::protocol::RateControl::Quality);
        let t = capture_target(None, None);
        assert_eq!((t.mode, t.monitor), (CaptureMode::Monitor, Some(0)));
    }
}
