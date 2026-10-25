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
    /// Constant quality (CQP on GPU encoders, CRF on x264) or constant bitrate. Older apps do not
    /// send it and get CBR, as before.
    #[serde(default)]
    pub rate_control: RateControl,
    /// Quality preset for [`RateControl::Quality`].
    #[serde(default)]
    pub quality: QualityPreset,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateControl {
    /// Constant bitrate at `bitrateKbps`.
    #[default]
    Cbr,
    /// Constant quality (CQP / CRF): small files in menus, sharp action.
    Quality,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    High,
    #[default]
    Medium,
    Low,
}

/// What the recorder learned about the machine's encoders, kept by the app between runs.
/// `failed` is only trusted while OBS still lists exactly `available` (a new GPU or driver
/// changes the list, and everything is tried again).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderMemory {
    /// OBS encoder ids listed when the memory was written.
    pub available: Vec<String>,
    /// Encoder ids that failed to start (or stalled) with that list.
    #[serde(default)]
    pub failed: Vec<String>,
    /// The encoder id that last recorded.
    #[serde(default)]
    pub working: Option<String>,
}

/// Frame statistics of one recording (deltas over the recording, not since OBS started).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingHealth {
    /// Frames written to the file.
    pub total_frames: u64,
    /// Frames the encoder skipped because it could not keep up, plus frames the output dropped.
    pub dropped_frames: u64,
    /// Frames OBS missed because rendering lagged (GPU busy).
    pub lagged_frames: u64,
    /// OBS encoder id that recorded.
    pub encoder: String,
    /// Why another encoder than the preferred one was used, if one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder_fallback: Option<String>,
    /// Seconds of footage from before the start request (the pre-roll), in ms.
    #[serde(default)]
    pub preroll_ms: u64,
}

impl RecordingHealth {
    /// Share of frames lost (dropped or lagged), 0..1. None without frames.
    pub fn lost_share(&self) -> Option<f32> {
        let lost = self.dropped_frames + self.lagged_frames;
        let total = self.total_frames + self.dropped_frames;
        (total > 0).then(|| (lost as f64 / total as f64).min(1.0) as f32)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioConfig {
    pub desktop: bool,
    pub microphone: bool,
    /// WASAPI device id, None = default device.
    #[serde(default)]
    pub microphone_device: Option<String>,
    /// Write the microphone alone to audio track 2 as well. Track 1 always carries the full mix
    /// (desktop + microphone), which is what players and browsers play.
    #[serde(default)]
    pub separate_mic_track: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RecorderCommand {
    /// Prepare for a game session: attach the capture, audio and encoder now and keep them for
    /// every recording until `Disarm`, so a match start only has to start (or keep) the file.
    /// With `bufferDir` and `prerollSecs` > 0 the output runs all the time into short rotating
    /// segments, and a `Start` keeps the segment in progress: the recording begins up to
    /// `prerollSecs` before the request. Replies with `Armed` once the capture is attached
    /// (which can be later, when the game window appears). Answers nothing while recording:
    /// the new settings apply after the recording stops.
    Arm {
        capture: CaptureTarget,
        video: VideoConfig,
        audio: AudioConfig,
        /// Folder for the rotating segments, on the same drive as the recordings (files are
        /// renamed, not copied, when a recording ends).
        #[serde(default)]
        buffer_dir: Option<String>,
        #[serde(default)]
        preroll_secs: u32,
        #[serde(default)]
        encoder_memory: Option<EncoderMemory>,
    },
    /// Release the capture and audio sources (the game exited). Replies with `Disarmed`.
    Disarm,
    /// Start recording to `path` (.mp4). Only one recording at a time. Reuses the armed capture
    /// when `capture`, `video` and `audio` match the last `Arm`; otherwise arms for this
    /// recording only.
    Start {
        id: String,
        path: String,
        capture: CaptureTarget,
        video: VideoConfig,
        audio: AudioConfig,
        #[serde(default)]
        encoder_memory: Option<EncoderMemory>,
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
    Ready {
        obs_version: String,
    },
    /// The capture for an `Arm` is attached (and, with a pre-roll, the buffer is running).
    Armed {
        capture: CaptureMode,
        /// Encoder the buffer runs on; None until a recording starts when there is no buffer.
        #[serde(default)]
        encoder: Option<String>,
        /// The pre-roll buffer is running.
        #[serde(default)]
        buffering: bool,
        #[serde(default)]
        encoder_fallback: Option<String>,
        #[serde(default)]
        encoder_memory: Option<EncoderMemory>,
    },
    Disarmed,
    /// Recording is writing frames. `startedUnixMs` is the wall-clock time of the first frame
    /// (best effort): with a pre-roll it is `prerollMs` before the start request.
    Started {
        id: String,
        path: String,
        started_unix_ms: u64,
        encoder: String,
        capture: CaptureMode,
        /// Footage kept from before the start request, in ms.
        #[serde(default)]
        preroll_ms: u64,
        /// Why another encoder than the preferred one is recording, if one is.
        #[serde(default)]
        encoder_fallback: Option<String>,
        #[serde(default)]
        encoder_memory: Option<EncoderMemory>,
    },
    /// Capture fell back to another mode (e.g. game hook failed -> monitor).
    CaptureChanged {
        id: String,
        capture: CaptureMode,
        reason: String,
    },
    Stopped {
        id: String,
        path: String,
        duration_ms: u64,
        #[serde(default)]
        health: Option<RecordingHealth>,
    },
    /// Something the player may want to know that does not stop the recording
    /// (`code`: "cpuEncoder", "captureRestarted", "encoderRestarted", "encoderSlow").
    Warning {
        #[serde(default)]
        id: Option<String>,
        code: String,
        message: String,
    },
    Encoders {
        encoders: Vec<String>,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_stable() {
        let c = RecorderCommand::Stop { id: "a".into() };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"cmd":"stop","id":"a"}"#);
        let e: RecorderEvent = serde_json::from_str(r#"{"event":"ready","obsVersion":"32.0.2"}"#).unwrap();
        assert_eq!(
            e,
            RecorderEvent::Ready {
                obs_version: "32.0.2".into()
            }
        );
        let s = RecorderCommand::Start {
            id: "m1".into(),
            path: "C:/v.mp4".into(),
            capture: CaptureTarget {
                mode: CaptureMode::Game,
                process: Some("x.exe".into()),
                pid: None,
                monitor: None,
            },
            video: VideoConfig {
                fps: 60,
                bitrate_kbps: 12000,
                output_height: None,
                encoder: "auto".into(),
                rate_control: RateControl::Quality,
                quality: QualityPreset::High,
            },
            audio: AudioConfig {
                desktop: true,
                microphone: true,
                microphone_device: None,
                separate_mic_track: true,
            },
            encoder_memory: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""rateControl":"quality","quality":"high""#), "{json}");
        assert!(serde_json::to_string(&s).unwrap().contains(r#""separateMicTrack":true"#));
        // Older apps do not send it: one mixed track.
        let old: AudioConfig = serde_json::from_str(r#"{"desktop":true,"microphone":true}"#).unwrap();
        assert!(!old.separate_mic_track);
        let back: RecorderCommand = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // Older apps: CBR, no memory.
        let old: VideoConfig = serde_json::from_str(r#"{"fps":60,"bitrateKbps":8000,"encoder":"auto"}"#).unwrap();
        assert_eq!((old.rate_control, old.quality), (RateControl::Cbr, QualityPreset::Medium));
        assert_eq!(serde_json::to_string(&RecorderCommand::Disarm).unwrap(), r#"{"cmd":"disarm"}"#);
        // Older recorders' events still parse.
        let st: RecorderEvent =
            serde_json::from_str(r#"{"event":"started","id":"a","path":"p","startedUnixMs":1,"encoder":"obs_x264","capture":"game"}"#)
                .unwrap();
        assert!(matches!(st, RecorderEvent::Started { preroll_ms: 0, .. }));
        let sp: RecorderEvent = serde_json::from_str(r#"{"event":"stopped","id":"a","path":"p","durationMs":5}"#).unwrap();
        assert!(matches!(sp, RecorderEvent::Stopped { health: None, .. }));
    }

    #[test]
    fn lost_share() {
        let h = RecordingHealth {
            total_frames: 90,
            dropped_frames: 10,
            lagged_frames: 5,
            ..Default::default()
        };
        assert_eq!(h.lost_share(), Some(0.15));
        assert_eq!(RecordingHealth::default().lost_share(), None);
    }
}
