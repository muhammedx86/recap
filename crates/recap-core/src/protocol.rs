//! JSON-lines protocol between the app and the `recap-recorder` sidecar.
//!
//! The app writes one [`RecorderCommand`] per line to the recorder's stdin; the
//! recorder writes one [`RecorderEvent`] per line to stdout. Logs go to stderr.

use serde::{Deserialize, Serialize};

use crate::games::CaptureMode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTarget {
    pub mode: CaptureMode,
    /// Executable name to capture (game/window modes), e.g. `VALORANT-Win64-Shipping.exe`.
    #[serde(default)]
    pub process: Option<String>,
    /// Process id, if known (more precise than the name).
    #[serde(default)]
    pub pid: Option<u32>,
    /// Monitor index for monitor mode (0 = primary).
    #[serde(default)]
    pub monitor: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConfig {
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Output height (width follows the aspect ratio). None = native.
    #[serde(default)]
    pub output_height: Option<u32>,
    /// "auto" | "nvenc" | "amf" | "qsv" | "x264"
    pub encoder: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioConfig {
    pub desktop: bool,
    pub microphone: bool,
    /// WASAPI device id, None = default device.
    #[serde(default)]
    pub microphone_device: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RecorderCommand {
    /// Start recording to `path` (.mp4). Only one recording at a time.
    Start {
        id: String,
        path: String,
        capture: CaptureTarget,
        video: VideoConfig,
        audio: AudioConfig,
    },
    /// Stop the current recording; replies with `Stopped`.
    Stop { id: String },
    /// List usable video encoders; replies with `Encoders`.
    ListEncoders,
    /// Finish any recording and exit.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RecorderEvent {
    /// OBS initialised and the recorder accepts commands.
    Ready { obs_version: String },
    /// Recording is writing frames. `startedUnixMs` is the wall-clock time of the first frame (best effort).
    Started { id: String, path: String, started_unix_ms: u64, encoder: String, capture: CaptureMode },
    /// Capture fell back to another mode (e.g. game hook failed -> monitor).
    CaptureChanged { id: String, capture: CaptureMode, reason: String },
    Stopped { id: String, path: String, duration_ms: u64 },
    Encoders { encoders: Vec<String> },
    Error { id: Option<String>, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_stable() {
        let c = RecorderCommand::Stop { id: "a".into() };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"cmd":"stop","id":"a"}"#);
        let e: RecorderEvent = serde_json::from_str(r#"{"event":"ready","obsVersion":"32.0.2"}"#).unwrap();
        assert_eq!(e, RecorderEvent::Ready { obs_version: "32.0.2".into() });
        let s = RecorderCommand::Start {
            id: "m1".into(),
            path: "C:/v.mp4".into(),
            capture: CaptureTarget { mode: CaptureMode::Game, process: Some("x.exe".into()), pid: None, monitor: None },
            video: VideoConfig { fps: 60, bitrate_kbps: 12000, output_height: None, encoder: "auto".into() },
            audio: AudioConfig { desktop: true, microphone: false, microphone_device: None },
        };
        let back: RecorderCommand = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
