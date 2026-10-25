//! The real recorder: a libobs context with one scene, one capture source, optional
//! desktop/mic audio and one MP4 output.
//!
//! Life cycle:
//! * `arm` (the game process appeared): find the game window, size the canvas for the monitor it
//!   is on (canvas = output size, so OBS scales once), attach capture and audio, and keep them
//!   for every match while the game runs. With a pre-roll the output also runs all the time into
//!   short rotating segments in a buffer folder ([`recap_core::prebuffer`]).
//! * `start`: with the buffer, the segment in progress becomes the recording (it began up to the
//!   pre-roll before the request); without it, the output starts writing the match file.
//! * `stop`: with the buffer, the output splits to a new segment and the finished one is renamed
//!   to the match path; without it, the output stops.
//! * `tick` (4x a second): picks up a late game window, rotates the buffer, restarts a capture
//!   that never shows a frame, moves to the next encoder if one never produces a frame, and
//!   follows the game window to another monitor between matches.
//!
//! Encoders are tried in [`plan_encoders`] order; starting the output is the test encode. A
//! failure is remembered by the app ([`EncoderMemory`]) until the machine's encoder list changes.
//!
//! libobs-wrapper marshals every OBS call onto its own runtime thread (`enable_runtime`), so
//! this type can be driven from the command loop thread.

use std::{
    ffi::CString,
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use libobs_wrapper::{
    context::ObsContext,
    data::{
        object::ObsObjectTrait,
        output::{ObsOutputRef, ObsOutputTrait},
        video::ObsVideoInfoBuilder,
        ObsData, ObsDataSetters,
    },
    encoders::{ObsAudioEncoderType, ObsContextEncoders, ObsVideoEncoderType},
    enums::ObsLogLevel,
    logger::ObsLogger,
    run_with_obs,
    runtime::ObsRuntime,
    scenes::{ObsSceneRef, SceneItemExtSceneTrait},
    sources::ObsSourceRef,
    unsafe_send::SmartPointerSendable,
    utils::{AudioEncoderInfo, ObsString, OutputInfo, SourceInfo, StartupInfo, VideoEncoderInfo},
};
use recap_core::{
    games::CaptureMode,
    prebuffer::{self, StartPlan},
    protocol::{AudioConfig, CaptureTarget, EncoderMemory, RecorderEvent, RecordingHealth, VideoConfig},
};

use crate::select::{
    encoder_settings, fallback_message, pick_monitor, plan_encoders, remembered_failures, updated_memory, EncodeAttempt, SettingValue,
};
use crate::session::{ArmRequest, Backend, StartError, StartOutcome, StartRequest, StopOutcome};

/// Audio tracks (OBS mixers, bit i = track i+1). Track 1 is the full mix that players play;
/// with a separate mic track, the microphone is also written alone to track 2.
const TRACK_1: u32 = 0b01;
const TRACKS_1_AND_2: u32 = 0b11;

/// How long to wait for the game window to appear before falling back to monitor capture.
const WINDOW_WAIT: Duration = Duration::from_secs(60);
/// A game capture that has not shown a frame this long after attaching is recreated...
const HOOK_GRACE: Duration = Duration::from_secs(20);
/// ...this many times, then the game's monitor is recorded instead.
const MAX_CAPTURE_RESTARTS: u32 = 2;
/// An output that wrote no frame this long after starting has a stuck encoder.
const ENCODER_STALL: Duration = Duration::from_secs(6);
/// How long a buffer split may take (it waits for the next keyframe, 1 s apart).
const SPLIT_WAIT: Duration = Duration::from_secs(10);
/// How often to check whether the game window moved to another monitor (between matches).
const MONITOR_CHECK: Duration = Duration::from_secs(5);
const OUTPUT_NAME: &str = "recap_output";

fn err<E: Display>(context: &str) -> impl Fn(E) -> anyhow::Error + '_ {
    move |e| anyhow::anyhow!("{context}: {e}")
}

/// Routes OBS logs to stderr (stdout carries the protocol).
#[derive(Debug)]
struct StderrLogger;

impl ObsLogger for StderrLogger {
    fn log(&mut self, level: ObsLogLevel, msg: String) {
        if level != ObsLogLevel::Debug {
            eprintln!("[obs {level}] {msg}");
        }
    }
}

fn log(msg: impl Display) {
    eprintln!("[recorder] {msg}");
}

// ---------------------------------------------------------------------------------------------
// Raw libobs calls the wrapper does not offer.

type SourcePtr = SmartPointerSendable<*mut libobs_wrapper::sys::obs_source_t>;

/// Route an audio source to the given tracks.
fn set_audio_tracks(source: &ObsSourceRef, mixers: u32) -> anyhow::Result<()> {
    let ptr = source.as_ptr();
    let runtime = source.runtime().clone();
    run_with_obs!(runtime, (ptr), move || {
        unsafe {
            // Safety: ptr is kept alive by the smart pointer for the duration of the call.
            libobs_wrapper::sys::obs_source_set_audio_mixers(ptr.get_ptr(), mixers);
        }
    })
    .map_err(err("setting audio tracks"))
}

/// Width of a source in pixels (0 while a game capture has not hooked).
fn source_width(src: &(SourcePtr, ObsRuntime)) -> u32 {
    let (ptr, runtime) = (src.0.clone(), src.1.clone());
    run_with_obs!(runtime, (ptr), move || {
        // Safety: the smart pointer keeps the source alive for the call.
        unsafe { libobs_wrapper::sys::obs_source_get_width(ptr.get_ptr()) }
    })
    .unwrap_or(0)
}

/// Change string/bool settings of an output, also while it runs (the hybrid MP4 output reads
/// `directory`/`format` when it splits).
fn set_output_values(output: &ObsOutputRef, strings: Vec<(&'static str, String)>, bools: Vec<(&'static str, bool)>) -> anyhow::Result<()> {
    let ptr = output.as_ptr();
    let runtime = output.runtime().clone();
    run_with_obs!(runtime, (ptr, strings, bools), move || {
        use libobs_wrapper::sys;
        // Safety: the output is alive (smart pointer); obs_output_get_settings returns a new
        // reference to its settings object, released below.
        unsafe {
            let data = sys::obs_output_get_settings(ptr.get_ptr());
            if data.is_null() {
                return;
            }
            for (k, v) in &strings {
                if let (Ok(k), Ok(v)) = (CString::new(*k), CString::new(v.as_str())) {
                    sys::obs_data_set_string(data, k.as_ptr(), v.as_ptr());
                }
            }
            for (k, v) in &bools {
                if let Ok(k) = CString::new(*k) {
                    sys::obs_data_set_bool(data, k.as_ptr(), *v);
                }
            }
            sys::obs_data_release(data);
        }
    })
    .map_err(err("output settings"))
}

/// Ask the hybrid MP4 output to start a new file at the next keyframe.
fn request_split(output: &ObsOutputRef) -> anyhow::Result<bool> {
    let ptr = output.as_ptr();
    let runtime = output.runtime().clone();
    run_with_obs!(runtime, (ptr), move || {
        use libobs_wrapper::sys;
        // Safety: the output is alive; a zeroed calldata is an empty one (what calldata_init
        // makes), and any stack the handler allocated is freed with bfree like calldata_free.
        unsafe {
            let ph = sys::obs_output_get_proc_handler(ptr.get_ptr());
            if ph.is_null() {
                return false;
            }
            let mut cd: sys::calldata_t = std::mem::zeroed();
            let ok = sys::proc_handler_call(ph, c"split_file".as_ptr(), &mut cd);
            if !cd.fixed && !cd.stack.is_null() {
                sys::bfree(cd.stack as *mut std::ffi::c_void);
            }
            ok
        }
    })
    .map_err(err("splitting the buffer"))
}

/// Frame counters, all cumulative (deltas give one recording's numbers).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Stats {
    /// Frames the output wrote (since the output started).
    total: u64,
    /// Frames the output dropped.
    dropped: u64,
    /// Frames OBS missed because rendering lagged (since OBS started).
    lagged: u64,
    /// Frames skipped because the encoder lagged (since the video started).
    skipped: u64,
}

fn stats(output: &ObsOutputRef) -> Stats {
    let ptr = output.as_ptr();
    let runtime = output.runtime().clone();
    run_with_obs!(runtime, (ptr), move || {
        use libobs_wrapper::sys;
        // Safety: the output is alive; obs_get_video may be null before video is set up.
        unsafe {
            let video = sys::obs_get_video();
            Stats {
                total: sys::obs_output_get_total_frames(ptr.get_ptr()).max(0) as u64,
                dropped: sys::obs_output_get_frames_dropped(ptr.get_ptr()).max(0) as u64,
                lagged: sys::obs_get_lagged_frames() as u64,
                skipped: if video.is_null() {
                    0
                } else {
                    sys::video_output_get_skipped_frames(video) as u64
                },
            }
        }
    })
    .unwrap_or_default()
}

fn set_value(data: &mut ObsData, key: &'static str, v: &SettingValue) -> anyhow::Result<()> {
    match v {
        SettingValue::Str(s) => data.set_string(key, *s).map(|_| ()),
        SettingValue::Int(i) => data.set_int(key, *i).map(|_| ()),
        SettingValue::Bool(b) => data.set_bool(key, *b).map(|_| ()),
    }
    .map_err(err("encoder settings"))
}

// ---------------------------------------------------------------------------------------------
// State.

/// What `arm` set up.
struct ArmState {
    capture: CaptureTarget,
    video: VideoConfig,
    audio: AudioConfig,
    buffer_dir: Option<PathBuf>,
    preroll_ms: u64,
    memory: Option<EncoderMemory>,
    /// Armed by a `Start` that did not match the last `Arm` (or came without one): released
    /// after that recording.
    temporary: bool,
    since: Instant,
    /// Output size/fps forced by an encoder fallback (x264 records at 720p).
    size: Option<(u32, u32, u32)>,
    attached: Option<Attached>,
    /// `Armed` was reported.
    reported: bool,
    last_monitor_check: Instant,
    /// After a failed attach, wait until then before trying again.
    retry_at: Option<Instant>,
}

/// The capture in the scene.
struct Attached {
    mode: CaptureMode,
    /// Why the capture differs from the requested mode.
    reason: Option<String>,
    /// Device name of the monitor the game window is on.
    window_monitor: Option<String>,
    /// The game capture source, watched until it shows a frame.
    source: Option<(SourcePtr, ObsRuntime)>,
    since: Instant,
    hooked: bool,
    restarts: u32,
}

/// The output is running.
struct Running {
    attempt: EncodeAttempt,
    first: EncodeAttempt,
    /// The rest of the plan, for a stalled encoder.
    rest: Vec<EncodeAttempt>,
    failed: Vec<String>,
    started: Instant,
    produced: bool,
    buffer: Option<Buffer>,
}

struct Buffer {
    dir: PathBuf,
    run: u64,
    /// Segment being written.
    n: u64,
    seg_started: Instant,
    /// A split to segment `.0` was requested at `.1`.
    split_to: Option<(u64, Instant)>,
}

impl Buffer {
    fn current(&self) -> PathBuf {
        prebuffer::segment_path(&self.dir, self.run, self.n)
    }
}

struct Rec {
    id: String,
    path: PathBuf,
    stats0: Stats,
    preroll_ms: u64,
    /// The buffer segment that is this recording.
    segment: Option<PathBuf>,
}

/// The last real `Arm`, restored after a temporary one.
#[derive(Clone)]
struct SavedArm {
    capture: CaptureTarget,
    video: VideoConfig,
    audio: AudioConfig,
    buffer_dir: Option<String>,
    preroll_secs: u32,
}

pub struct ObsBackend {
    ctx: ObsContext,
    scene: ObsSceneRef,
    output: Option<ObsOutputRef>,
    /// (fps, width, height) currently applied (canvas = output).
    video: Option<(u32, u32, u32)>,
    arm: Option<ArmState>,
    run: Option<Running>,
    rec: Option<Rec>,
    saved: Option<SavedArm>,
    /// Encoder memory as last reported (survives re-arms within this process).
    memory: Option<EncoderMemory>,
}

fn same_setup(a: &ArmState, capture: &CaptureTarget, video: &VideoConfig, audio: &AudioConfig) -> bool {
    let same_process = match (&a.capture.process, &capture.process) {
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        (x, y) => x == y,
    };
    a.capture.mode == capture.mode && same_process && a.capture.monitor == capture.monitor && a.video == *video && a.audio == *audio
}

impl ObsBackend {
    /// Start OBS. Returns the backend and the libobs version string.
    pub fn new() -> anyhow::Result<(Self, String)> {
        let ovi = ObsVideoInfoBuilder::new().build();
        let mut ctx = StartupInfo::new()
            .set_video_info(ovi)
            .set_logger(Box::new(StderrLogger))
            .start()
            .map_err(err("starting OBS"))?;
        let version = ctx.get_version().map_err(err("reading OBS version"))?;
        let scene = ctx.scene("recap", Some(0)).map_err(err("creating scene"))?;
        Ok((
            Self {
                ctx,
                scene,
                output: None,
                video: None,
                arm: None,
                run: None,
                rec: None,
                saved: None,
                memory: None,
            },
            version,
        ))
    }

    /// Canvas = output = (w, h) at `fps`: OBS scales each source once, into the canvas.
    fn configure_video(&mut self, fps: u32, w: u32, h: u32) -> anyhow::Result<()> {
        let fps = fps.clamp(10, 240);
        if self.video == Some((fps, w, h)) {
            return Ok(());
        }
        let ovi = ObsVideoInfoBuilder::new()
            .fps_num(fps)
            .fps_den(1)
            .base_width(w)
            .base_height(h)
            .output_width(w)
            .output_height(h)
            .build();
        self.ctx.reset_video(ovi).map_err(err("applying video settings"))?;
        self.video = Some((fps, w, h));
        Ok(())
    }

    /// Add audio sources requested by the app to the scene.
    fn add_audio(&mut self, audio: &AudioConfig) -> anyhow::Result<()> {
        // A separate mic track only makes sense when the mic is recorded.
        let mic_track = audio.microphone && audio.separate_mic_track;
        if audio.desktop {
            let mut data = self.ctx.data().map_err(err("audio settings"))?;
            data.set_string("device_id", "default").map_err(err("audio settings"))?;
            let item = self
                .scene
                .add_and_create_source(SourceInfo::new("wasapi_output_capture", "recap_desktop", Some(data), None))
                .map_err(err("adding desktop audio"))?;
            set_audio_tracks(item.inner_source(), TRACK_1)?;
        }
        if audio.microphone {
            let mut data = self.ctx.data().map_err(err("mic settings"))?;
            data.set_string("device_id", audio.microphone_device.as_deref().unwrap_or("default"))
                .map_err(err("mic settings"))?;
            let item = self
                .scene
                .add_and_create_source(SourceInfo::new("wasapi_input_capture", "recap_mic", Some(data), None))
                .map_err(err("adding microphone"))?;
            set_audio_tracks(item.inner_source(), if mic_track { TRACKS_1_AND_2 } else { TRACK_1 })?;
        }
        Ok(())
    }

    fn clear_scene(&mut self) {
        if let Err(e) = self.scene.remove_all_sources() {
            log(format!("clearing scene failed: {e}"));
        }
    }

    fn available_encoders(&mut self) -> Vec<String> {
        self.encoders().unwrap_or_default()
    }

    /// The encoder plan for the armed setup on a canvas of `canvas`.
    fn plan(&mut self, canvas: (u32, u32)) -> (Vec<EncodeAttempt>, Vec<String>) {
        let available = self.available_encoders();
        let Some(a) = self.arm.as_ref() else { return (vec![], available) };
        let skip = remembered_failures(a.memory.as_ref().or(self.memory.as_ref()), &available);
        let plan = plan_encoders(&a.video.encoder, &available, &skip, canvas, a.video.output_height, a.video.fps);
        (plan, available)
    }

    // --- capture --------------------------------------------------------------------------

    /// Attach the capture and audio for the armed setup if the game window is there (or the
    /// wait is over: then the monitor). Returns true once attached.
    fn try_attach(&mut self, give_up_waiting: bool) -> Result<bool, String> {
        let Some(a) = self.arm.as_ref() else { return Ok(false) };
        if a.attached.is_some() {
            return Ok(true);
        }
        let target = a.capture.clone();
        let audio = a.audio.clone();
        let waited = a.since.elapsed();

        let window = if target.mode == CaptureMode::Monitor {
            None
        } else {
            match capture::find_window(target.process.as_deref(), target.pid) {
                Some(w) => Some(w),
                None if give_up_waiting || waited >= WINDOW_WAIT => None,
                None => return Ok(false),
            }
        };
        let window_monitor = window.as_ref().and_then(capture::window_monitor);
        let monitors = capture::monitors();
        let mi = pick_monitor(&monitors, window_monitor.as_deref(), target.monitor);
        let canvas = mi.map(|i| (monitors[i].width, monitors[i].height)).unwrap_or_else(|| {
            let probe = ObsVideoInfoBuilder::new().build();
            (probe.get_base_width(), probe.get_base_height())
        });

        // Size the canvas for the encoder that will record (the first of the plan, or the one a
        // fallback forced).
        let size = match self.arm.as_ref().and_then(|a| a.size) {
            Some(s) => s,
            None => {
                let (plan, _) = self.plan(canvas);
                let first = plan.first().cloned();
                first.map_or((self.arm.as_ref().map_or(60, |a| a.video.fps), canvas.0, canvas.1), |p| {
                    (p.fps, p.width, p.height)
                })
            }
        };
        self.clear_scene();
        self.configure_video(size.0, size.1, size.2).map_err(|e| format!("{e:#}"))?;

        let (mode, reason, source) = match (&window, target.mode) {
            (Some(w), CaptureMode::Game) => (CaptureMode::Game, None, Some(capture::add_game(self, w)?)),
            (Some(w), _) => {
                capture::add_window(self, w)?;
                (CaptureMode::Window, None, None)
            }
            (None, CaptureMode::Monitor) => {
                capture::add_monitor(self, mi)?;
                (CaptureMode::Monitor, None, None)
            }
            (None, _) => {
                capture::add_monitor(self, mi)?;
                let what = target
                    .process
                    .clone()
                    .or(target.pid.map(|p| format!("pid {p}")))
                    .unwrap_or_else(|| "the game".into());
                let reason = format!("no window for {what} after {}s; recording the monitor", waited.as_secs());
                (CaptureMode::Monitor, Some(reason), None)
            }
        };
        self.add_audio(&audio).map_err(|e| format!("{e:#}"))?;
        if let Some(a) = self.arm.as_mut() {
            a.attached = Some(Attached {
                mode,
                reason,
                window_monitor,
                source,
                since: Instant::now(),
                hooked: false,
                restarts: 0,
            });
            a.last_monitor_check = Instant::now();
        }
        Ok(true)
    }

    /// Drop the capture (the scene is rebuilt by the next `try_attach`).
    fn detach(&mut self) {
        if let Some(a) = self.arm.as_mut() {
            a.attached = None;
        }
        self.clear_scene();
    }

    // --- output ---------------------------------------------------------------------------

    /// Set up encoders for `attempt` and start the output into `path` (splittable for the
    /// buffer).
    fn start_output_with(&mut self, attempt: &EncodeAttempt, path: &Path, split: Option<&Buffer>) -> anyhow::Result<()> {
        let (video, audio) = match self.arm.as_ref() {
            Some(a) => (a.video.clone(), a.audio.clone()),
            None => anyhow::bail!("not armed"),
        };
        let mic_track = audio.microphone && audio.separate_mic_track;
        let mut out_settings = self.ctx.data().map_err(err("output settings"))?;
        out_settings
            .set_string("path", path.to_string_lossy().as_ref())
            .map_err(err("output settings"))?;
        out_settings
            .set_bool("split_file", split.is_some())
            .map_err(err("output settings"))?;
        if let Some(b) = split {
            out_settings
                .set_string("directory", b.dir.to_string_lossy().as_ref())
                .map_err(err("output settings"))?;
            out_settings
                .set_string("format", prebuffer::segment_stem(b.run, b.n + 1))
                .map_err(err("output settings"))?;
            out_settings.set_string("extension", "mp4").map_err(err("output settings"))?;
            out_settings.set_bool("allow_overwrite", true).map_err(err("output settings"))?;
            out_settings.set_bool("allow_spaces", false).map_err(err("output settings"))?;
        }
        if self.output.is_none() {
            // mp4_output is OBS's hybrid MP4 muxer: the file stays playable if we crash, and it
            // can split files without a gap (the pre-roll buffer).
            let output = self
                .ctx
                .output(OutputInfo::new("mp4_output", OUTPUT_NAME, Some(out_settings), None))
                .map_err(err("creating output"))?;
            self.output = Some(output);
        } else {
            self.ctx.update_output(OUTPUT_NAME, out_settings).map_err(err("updating output"))?;
        }

        let mut vs = self.ctx.data().map_err(err("encoder settings"))?;
        for (k, v) in encoder_settings(&attempt.encoder, &video) {
            set_value(&mut vs, k, &v)?;
        }
        let mut aud = self.ctx.data().map_err(err("audio encoder settings"))?;
        aud.set_int("bitrate", 160).map_err(err("audio encoder settings"))?;

        let output = self.output.as_mut().expect("output created above");
        let enc_type = ObsVideoEncoderType::from_str(&attempt.encoder).expect("infallible");
        output
            .create_and_set_video_encoder(VideoEncoderInfo::new(enc_type, "recap_video_encoder", Some(vs), None))
            .map_err(err("creating video encoder"))?;
        output
            .create_and_set_audio_encoder(
                AudioEncoderInfo::new(ObsAudioEncoderType::FFMPEG_AAC, "recap_audio_encoder", Some(aud), None),
                0,
            )
            .map_err(err("creating audio encoder"))?;
        let has_mic_track = output.audio_encoders().read().map(|m| m.contains_key(&1)).unwrap_or(false);
        if mic_track {
            // Track 2: the microphone alone (mixer 1).
            let mut mic_aud = self.ctx.data().map_err(err("mic encoder settings"))?;
            mic_aud.set_int("bitrate", 128).map_err(err("mic encoder settings"))?;
            output
                .create_and_set_audio_encoder(
                    AudioEncoderInfo::new(ObsAudioEncoderType::FFMPEG_AAC, "recap_audio_encoder_mic", Some(mic_aud), None),
                    1,
                )
                .map_err(err("creating mic track encoder"))?;
        } else if has_mic_track {
            // The output is reused: drop the mic track left over from an earlier recording.
            let out_ptr = output.as_ptr();
            let runtime = output.runtime().clone();
            run_with_obs!(runtime, (out_ptr), move || {
                unsafe {
                    // Safety: out_ptr is kept alive by the smart pointer; a null encoder detaches track 2.
                    libobs_wrapper::sys::obs_output_set_audio_encoder(out_ptr.get_ptr(), std::ptr::null_mut(), 1);
                }
            })
            .map_err(err("removing mic track"))?;
            if let Ok(mut m) = output.audio_encoders().write() {
                m.remove(&1);
            }
        }
        output.start().map_err(err("starting output"))?;
        Ok(())
    }

    fn output_active(&self) -> bool {
        self.output.as_ref().is_some_and(|o| o.is_active().unwrap_or(false))
    }

    fn stop_output(&mut self) -> anyhow::Result<()> {
        match self.output.as_mut() {
            Some(o) if o.is_active().unwrap_or(false) => o.stop().map_err(err("stopping output")),
            _ => Ok(()),
        }
    }

    /// Start the output, trying the encoder plan in order (each start is the test encode).
    /// `path` None = into a new buffer run. Returns notices to emit.
    fn launch(
        &mut self,
        path: Option<&Path>,
        plan: Vec<EncodeAttempt>,
        mut failed: Vec<String>,
        preferred: Option<EncodeAttempt>,
    ) -> Result<Vec<RecorderEvent>, String> {
        let Some(first) = preferred.or_else(|| plan.first().cloned()) else {
            return Err("no encoder to record with".into());
        };
        let available = self.available_encoders();
        let mut notices = Vec::new();
        let mut rest: Vec<EncodeAttempt> = plan;
        let mut last_err = String::new();
        while !rest.is_empty() {
            let attempt = rest.remove(0);
            // A fallback with another size (x264 at 720p) needs the canvas rebuilt.
            let size = (attempt.fps, attempt.width, attempt.height);
            if self.video != Some((attempt.fps.clamp(10, 240), attempt.width, attempt.height)) {
                if let Some(a) = self.arm.as_mut() {
                    a.size = Some(size);
                }
                self.detach();
                if let Err(e) = self.try_attach(true) {
                    last_err = e;
                    continue;
                }
            }
            let buffer = match (path, self.arm.as_ref().and_then(|a| a.buffer_dir.clone())) {
                (None, Some(dir)) => Some(Buffer {
                    dir,
                    run: session_run_id(),
                    n: 0,
                    seg_started: Instant::now(),
                    split_to: None,
                }),
                _ => None,
            };
            let target = match (&buffer, path) {
                (Some(b), _) => b.current(),
                (None, Some(p)) => p.to_path_buf(),
                (None, None) => return Err("no buffer folder".into()),
            };
            match self.start_output_with(&attempt, &target, buffer.as_ref()) {
                Ok(()) => {
                    log(format!(
                        "recording with {} at {}x{}@{} into {}",
                        attempt.encoder,
                        attempt.width,
                        attempt.height,
                        attempt.fps,
                        target.display()
                    ));
                    if attempt.cpu_fallback {
                        notices.push(RecorderEvent::Warning {
                            id: None,
                            code: "cpuEncoder".into(),
                            message: fallback_message(&first, &attempt, &failed).unwrap_or_default(),
                        });
                    }
                    let memory = updated_memory(
                        self.arm.as_ref().and_then(|a| a.memory.as_ref()).or(self.memory.as_ref()),
                        &available,
                        &failed,
                        Some(&attempt.encoder),
                    );
                    self.memory = Some(memory.clone());
                    if let Some(a) = self.arm.as_mut() {
                        a.memory = Some(memory);
                    }
                    let mut buffer = buffer;
                    if let Some(b) = buffer.as_mut() {
                        b.seg_started = Instant::now();
                    }
                    self.run = Some(Running {
                        attempt,
                        first,
                        rest,
                        failed,
                        started: Instant::now(),
                        produced: false,
                        buffer,
                    });
                    return Ok(notices);
                }
                Err(e) => {
                    log(format!("{} did not start: {e:#}", attempt.encoder));
                    last_err = format!("{e:#}");
                    if !failed.contains(&attempt.encoder) {
                        failed.push(attempt.encoder.clone());
                    }
                }
            }
        }
        let memory = updated_memory(self.memory.as_ref(), &available, &failed, None);
        self.memory = Some(memory);
        Err(format!("no encoder could start ({last_err})"))
    }

    fn launch_plan(&mut self, path: Option<&Path>) -> Result<Vec<RecorderEvent>, String> {
        let canvas = self
            .arm
            .as_ref()
            .and_then(|a| a.size)
            .map(|(_, w, h)| (w, h))
            .or(self.video.map(|(_, w, h)| (w, h)))
            .unwrap_or((1920, 1080));
        // The plan is made for the monitor size (the canvas may already be scaled down).
        let canvas = self.monitor_canvas().unwrap_or(canvas);
        let (plan, _) = self.plan(canvas);
        self.launch(path, plan, vec![], None)
    }

    /// Size of the monitor the capture is sized for.
    fn monitor_canvas(&self) -> Option<(u32, u32)> {
        let a = self.arm.as_ref()?;
        let monitors = capture::monitors();
        let wm = a.attached.as_ref().and_then(|t| t.window_monitor.clone());
        let i = pick_monitor(&monitors, wm.as_deref(), a.capture.monitor)?;
        Some((monitors[i].width, monitors[i].height))
    }

    // --- buffer ---------------------------------------------------------------------------

    fn buffering(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.buffer.is_some())
    }

    /// Start the pre-roll buffer if the arm wants one and the capture is attached.
    fn start_buffer(&mut self) -> Vec<RecorderEvent> {
        let wants = self
            .arm
            .as_ref()
            .is_some_and(|a| !a.temporary && a.buffer_dir.is_some() && a.preroll_ms > 0 && a.attached.is_some());
        if !wants || self.run.is_some() {
            return vec![];
        }
        match self.launch_plan(None) {
            Ok(n) => n,
            Err(e) => vec![RecorderEvent::Warning {
                id: None,
                code: "bufferFailed".into(),
                message: format!("Could not start the pre-roll buffer: {e}"),
            }],
        }
    }

    /// Request a split to the next segment (no-op while one is pending).
    fn rotate(&mut self) -> bool {
        let (Some(run), Some(output)) = (self.run.as_mut(), self.output.as_ref()) else {
            return false;
        };
        let Some(b) = run.buffer.as_mut() else { return false };
        if b.split_to.is_some() {
            return true;
        }
        let next = b.n + 1;
        let set = set_output_values(
            output,
            vec![
                ("directory", b.dir.to_string_lossy().into_owned()),
                ("format", prebuffer::segment_stem(b.run, next)),
                ("extension", "mp4".into()),
            ],
            vec![("allow_overwrite", true), ("allow_spaces", false)],
        );
        match set.and_then(|_| request_split(output)) {
            Ok(true) => {
                b.split_to = Some((next, Instant::now()));
                true
            }
            Ok(false) => {
                log("the output refused to split");
                false
            }
            Err(e) => {
                log(format!("{e:#}"));
                false
            }
        }
    }

    /// If a pending split finished, advance to the new segment and return the finished one.
    /// Err when the split is overdue.
    fn split_done(&mut self) -> Result<Option<PathBuf>, ()> {
        let Some(b) = self.run.as_mut().and_then(|r| r.buffer.as_mut()) else {
            return Ok(None);
        };
        let Some((next, since)) = b.split_to else { return Ok(None) };
        // The new file is opened only after the old one is finalised and closed.
        if prebuffer::segment_path(&b.dir, b.run, next).exists() {
            let old = b.current();
            b.n = next;
            b.seg_started = Instant::now();
            b.split_to = None;
            return Ok(Some(old));
        }
        if since.elapsed() > SPLIT_WAIT {
            return Err(());
        }
        Ok(None)
    }

    /// Wait (polling) for a pending split. Returns the finished segment.
    fn wait_split(&mut self) -> Result<Option<PathBuf>, ()> {
        loop {
            match self.split_done()? {
                Some(p) => return Ok(Some(p)),
                None if self
                    .run
                    .as_ref()
                    .and_then(|r| r.buffer.as_ref())
                    .is_some_and(|b| b.split_to.is_some()) =>
                {
                    thread::sleep(Duration::from_millis(50))
                }
                None => return Ok(None),
            }
        }
    }

    /// Buffer housekeeping between recordings: delete finished segments, rotate.
    fn tick_buffer(&mut self) -> Vec<RecorderEvent> {
        if self.rec.is_some() || !self.buffering() {
            return vec![];
        }
        match self.split_done() {
            Ok(Some(old)) => {
                if !prebuffer::marker_path(&old).exists() {
                    let _ = std::fs::remove_file(&old);
                }
            }
            Ok(None) => {}
            Err(()) => {
                log("the buffer did not split in time; restarting it");
                return self.restart_buffer();
            }
        }
        let rotate = {
            let (Some(a), Some(b)) = (self.arm.as_ref(), self.run.as_ref().and_then(|r| r.buffer.as_ref())) else {
                return vec![];
            };
            b.split_to.is_none() && prebuffer::should_rotate(b.seg_started.elapsed().as_millis() as u64, a.preroll_ms)
        };
        if rotate {
            self.rotate();
        }
        vec![]
    }

    fn restart_buffer(&mut self) -> Vec<RecorderEvent> {
        let _ = self.stop_output();
        self.run = None;
        if let Some(dir) = self.arm.as_ref().and_then(|a| a.buffer_dir.clone()) {
            prebuffer::clean(&dir);
        }
        self.start_buffer()
    }

    // --- arm / disarm ---------------------------------------------------------------------

    fn arm_with(&mut self, setup: &SavedArm, memory: Option<&EncoderMemory>, temporary: bool) {
        self.disarm_inner();
        let buffer_dir = setup.buffer_dir.as_deref().filter(|d| !d.is_empty()).map(PathBuf::from);
        if let Some(d) = &buffer_dir {
            if let Err(e) = std::fs::create_dir_all(d) {
                log(format!("buffer folder {}: {e}", d.display()));
            }
            // Recordings a crash left in the buffer go where they belong; the rest is junk.
            for t in prebuffer::recover(d) {
                log(format!("recovered {}", t.display()));
            }
            prebuffer::clean(d);
        }
        if memory.is_some() {
            self.memory = memory.cloned();
        }
        self.arm = Some(ArmState {
            capture: setup.capture.clone(),
            video: setup.video.clone(),
            audio: setup.audio.clone(),
            buffer_dir,
            preroll_ms: setup.preroll_secs.min(120) as u64 * 1000,
            memory: memory.cloned().or(self.memory.clone()),
            temporary,
            since: Instant::now(),
            size: None,
            attached: None,
            reported: false,
            last_monitor_check: Instant::now(),
            retry_at: None,
        });
    }

    fn disarm_inner(&mut self) {
        if let Err(e) = self.stop_output() {
            log(format!("{e:#}"));
        }
        let dir = self.run.as_ref().and_then(|r| r.buffer.as_ref()).map(|b| b.dir.clone());
        self.run = None;
        if let Some(d) = dir {
            prebuffer::clean(&d);
        }
        self.clear_scene();
        self.arm = None;
    }

    /// Attach if possible, start the buffer, and report `Armed` once.
    fn settle_arm(&mut self) -> Vec<RecorderEvent> {
        let mut out = Vec::new();
        match self.try_attach(false) {
            Ok(true) => {}
            Ok(false) => return out,
            Err(e) => {
                out.push(RecorderEvent::Warning {
                    id: None,
                    code: "captureFailed".into(),
                    message: format!("Could not attach the capture: {e}"),
                });
                // Try again from scratch a little later.
                self.detach();
                if let Some(a) = self.arm.as_mut() {
                    a.since = Instant::now();
                    a.retry_at = Some(Instant::now() + Duration::from_secs(10));
                }
                return out;
            }
        }
        out.extend(self.start_buffer());
        out.extend(self.armed_event());
        out
    }

    /// `Armed`, once per arm, when the capture is attached and no recording runs (a recording
    /// that started before the window appeared reports it after it stops).
    fn armed_event(&mut self) -> Option<RecorderEvent> {
        if self.rec.is_some() {
            return None;
        }
        let buffering = self.buffering();
        let encoder = self.run.as_ref().map(|r| r.attempt.encoder.clone());
        let encoder_fallback = self.run.as_ref().and_then(|r| fallback_message(&r.first, &r.attempt, &r.failed));
        let a = self.arm.as_mut().filter(|a| !a.reported && !a.temporary && a.attached.is_some())?;
        a.reported = true;
        Some(RecorderEvent::Armed {
            capture: a.attached.as_ref().map_or(a.capture.mode, |t| t.mode),
            encoder,
            buffering,
            encoder_fallback,
            encoder_memory: a.memory.clone(),
        })
    }

    // --- watchdogs ------------------------------------------------------------------------

    /// A game capture that never showed a frame is recreated, then replaced by the monitor.
    fn watch_capture(&mut self) -> Vec<RecorderEvent> {
        let rec_id = self.rec.as_ref().map(|r| r.id.clone());
        let Some(t) = self.arm.as_mut().and_then(|a| a.attached.as_mut()) else {
            return vec![];
        };
        let Some(src) = t.source.as_ref() else { return vec![] };
        if t.hooked {
            return vec![];
        }
        if source_width(src) > 0 {
            t.hooked = true;
            // Nothing more to watch: game capture re-hooks by itself after alt-tab or a new window.
            t.source = None;
            return vec![];
        }
        if t.since.elapsed() < HOOK_GRACE {
            return vec![];
        }
        let restarts = t.restarts;
        // Rebuilding the scene while a recording writes is fine: it is black anyway.
        if restarts < MAX_CAPTURE_RESTARTS {
            log("game capture shows no frames; recreating it");
            let was_hooked_restarts = restarts + 1;
            let live = self.output_active();
            // The canvas size stays (the output may be running), so keep the forced size.
            let cur = self.video;
            if let Some(a) = self.arm.as_mut() {
                a.size = cur;
            }
            if live {
                self.rebuild_scene_live();
            } else {
                self.detach();
                let _ = self.try_attach(true);
            }
            if let Some(t) = self.arm.as_mut().and_then(|a| a.attached.as_mut()) {
                t.restarts = was_hooked_restarts;
            }
            return vec![RecorderEvent::Warning {
                id: rec_id,
                code: "captureRestarted".into(),
                message: "The game capture showed no picture; it was restarted".into(),
            }];
        }
        // Give up on the hook: record the game's monitor.
        log("game capture never showed a frame; recording the monitor");
        let monitors = capture::monitors();
        let (mi, wm) = {
            let a = self.arm.as_ref().expect("armed");
            let wm = a.attached.as_ref().and_then(|t| t.window_monitor.clone());
            (pick_monitor(&monitors, wm.as_deref(), a.capture.monitor), wm)
        };
        self.clear_scene();
        let audio = self.arm.as_ref().map(|a| a.audio.clone());
        let ok = capture::add_monitor(self, mi).is_ok();
        if let Some(audio) = audio {
            let _ = self.add_audio(&audio);
        }
        let reason = "the game capture never showed a picture; recording the monitor".to_string();
        if let Some(a) = self.arm.as_mut() {
            a.attached = Some(Attached {
                mode: CaptureMode::Monitor,
                reason: Some(reason.clone()),
                window_monitor: wm,
                source: None,
                since: Instant::now(),
                hooked: true,
                restarts,
            });
        }
        if !ok {
            return vec![];
        }
        match rec_id {
            Some(id) => vec![RecorderEvent::CaptureChanged {
                id,
                capture: CaptureMode::Monitor,
                reason,
            }],
            None => vec![RecorderEvent::Warning {
                id: None,
                code: "captureChanged".into(),
                message: reason,
            }],
        }
    }

    /// Recreate capture and audio while the output runs (canvas size unchanged).
    fn rebuild_scene_live(&mut self) {
        let Some(a) = self.arm.as_ref() else { return };
        let target = a.capture.clone();
        let audio = a.audio.clone();
        let wm = a.attached.as_ref().and_then(|t| t.window_monitor.clone());
        let restarts = a.attached.as_ref().map_or(0, |t| t.restarts);
        self.clear_scene();
        let window = capture::find_window(target.process.as_deref(), target.pid);
        let source = match (&window, target.mode) {
            (Some(w), CaptureMode::Game) => capture::add_game(self, w).ok(),
            (Some(w), _) => {
                let _ = capture::add_window(self, w);
                None
            }
            (None, _) => {
                let monitors = capture::monitors();
                let _ = capture::add_monitor(self, pick_monitor(&monitors, wm.as_deref(), target.monitor));
                None
            }
        };
        let _ = self.add_audio(&audio);
        if let Some(t) = self.arm.as_mut().and_then(|a| a.attached.as_mut()) {
            t.hooked = source.is_none();
            t.source = source;
            t.since = Instant::now();
            t.restarts = restarts;
        }
    }

    /// An encoder that writes no frames is replaced by the next one of the plan.
    fn watch_encoder(&mut self) -> Vec<RecorderEvent> {
        let Some(run) = self.run.as_mut() else { return vec![] };
        if run.produced {
            return vec![];
        }
        let Some(output) = self.output.as_ref() else { return vec![] };
        if stats(output).total > 0 {
            run.produced = true;
            return vec![];
        }
        if run.started.elapsed() < ENCODER_STALL {
            return vec![];
        }
        let bad = run.attempt.encoder.clone();
        log(format!(
            "{bad} wrote no frames in {}s; trying the next encoder",
            ENCODER_STALL.as_secs()
        ));
        let mut failed = run.failed.clone();
        if !failed.contains(&bad) {
            failed.push(bad.clone());
        }
        let rest = run.rest.clone();
        let first = run.first.clone();
        let buffered = run.buffer.is_some();
        let _ = self.stop_output();
        self.run = None;
        let rec_id = self.rec.as_ref().map(|r| r.id.clone());
        // The rest of the plan, without anything that failed.
        let plan: Vec<EncodeAttempt> = rest.into_iter().filter(|p| !failed.contains(&p.encoder)).collect();
        let path = match (&self.rec, buffered) {
            (Some(r), false) => Some(r.path.clone()),
            _ => None,
        };
        if let Some(dir) = self
            .arm
            .as_ref()
            .and_then(|a| a.buffer_dir.clone())
            .filter(|_| buffered && self.rec.is_none())
        {
            prebuffer::clean(&dir);
        }
        let mut out = vec![RecorderEvent::Warning {
            id: rec_id.clone(),
            code: "encoderRestarted".into(),
            message: format!("{bad} produced no video; switched encoder"),
        }];
        match self.launch(path.as_deref(), plan, failed, Some(first)) {
            Ok(n) => {
                out.extend(n);
                // A buffered recording continues in the new run's first segment.
                if let (Some(r), Some(b)) = (self.rec.as_mut(), self.run.as_ref().and_then(|r| r.buffer.as_ref())) {
                    let seg = b.current();
                    let _ = prebuffer::write_marker(&seg, &r.path);
                    r.segment = Some(seg);
                }
            }
            Err(e) => out.push(RecorderEvent::Error {
                id: rec_id,
                message: format!("Recording stopped: {e}"),
            }),
        }
        out
    }

    /// Between matches, follow the game window to another monitor (new canvas size).
    fn watch_monitor(&mut self) -> Vec<RecorderEvent> {
        if self.rec.is_some() {
            return vec![];
        }
        let Some(a) = self.arm.as_mut() else { return vec![] };
        if a.last_monitor_check.elapsed() < MONITOR_CHECK {
            return vec![];
        }
        a.last_monitor_check = Instant::now();
        let Some(t) = a.attached.as_ref() else { return vec![] };
        if t.mode == CaptureMode::Monitor {
            return vec![];
        }
        let Some(w) = capture::find_window(a.capture.process.as_deref(), a.capture.pid) else {
            return vec![];
        };
        let now = capture::window_monitor(&w);
        if now.is_none() || now == t.window_monitor {
            return vec![];
        }
        log(format!("the game moved to {}; resizing the canvas", now.as_deref().unwrap_or("?")));
        let buffered = self.buffering();
        let _ = self.stop_output();
        self.run = None;
        if let Some(a) = self.arm.as_mut() {
            a.size = None;
        }
        self.detach();
        let mut out = self.settle_arm();
        if !buffered {
            out.retain(|e| !matches!(e, RecorderEvent::Armed { .. }));
        }
        out
    }

    // --- recording ------------------------------------------------------------------------

    fn start_inner(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError> {
        let failed = |e: String| StartError::Failed(e);
        let mut notices = Vec::new();
        let reuse = self.arm.as_ref().is_some_and(|a| same_setup(a, req.capture, req.video, req.audio));
        if !reuse {
            // Arm for this recording only (no buffer); the real arm comes back after it.
            let setup = SavedArm {
                capture: req.capture.clone(),
                video: req.video.clone(),
                audio: req.audio.clone(),
                buffer_dir: None,
                preroll_secs: 0,
            };
            self.arm_with(&setup, req.memory, true);
        } else if let (Some(a), Some(m)) = (self.arm.as_mut(), req.memory) {
            if a.memory.is_none() {
                a.memory = Some(m.clone());
            }
        }

        // Wait for the game window (the arm may still be waiting for it).
        loop {
            match self.try_attach(false) {
                Ok(true) => break,
                Ok(false) => {
                    if sleep_abortable(Duration::from_millis(500), abort) {
                        return Err(StartError::Aborted);
                    }
                }
                Err(e) => return Err(failed(e)),
            }
        }
        let (capture, fallback_reason) = {
            let t = self.arm.as_ref().and_then(|a| a.attached.as_ref()).expect("attached");
            (t.mode, t.reason.clone())
        };
        let path = PathBuf::from(req.path);
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }

        // With the buffer: the current segment becomes the recording. Finish a rotation in
        // flight first (its old segment is not part of the recording).
        if reuse && self.buffering() {
            match self.wait_split() {
                Ok(Some(old)) => {
                    let _ = std::fs::remove_file(old);
                }
                Ok(None) => {}
                Err(()) => {
                    log("buffer split overdue at start; recording without pre-roll");
                    let _ = self.stop_output();
                    self.run = None;
                }
            }
        }
        if reuse && self.buffering() {
            let (age, preroll) = {
                let b = self.run.as_ref().and_then(|r| r.buffer.as_ref()).expect("buffering");
                let a = self.arm.as_ref().expect("armed");
                (b.seg_started.elapsed().as_millis() as u64, a.preroll_ms)
            };
            let preroll_ms = match prebuffer::plan_start(age, preroll) {
                StartPlan::KeepCurrent { preroll_ms } => preroll_ms,
                StartPlan::SplitFirst => {
                    if self.rotate() {
                        match self.wait_split() {
                            Ok(Some(old)) => {
                                let _ = std::fs::remove_file(old);
                            }
                            _ => log("buffer split overdue at start"),
                        }
                    }
                    0
                }
            };
            if self.buffering() {
                let seg = self
                    .run
                    .as_ref()
                    .and_then(|r| r.buffer.as_ref())
                    .map(|b| b.current())
                    .expect("buffering");
                if let Err(e) = prebuffer::write_marker(&seg, &path) {
                    log(format!("marker for {}: {e}", seg.display()));
                }
                let run = self.run.as_ref().expect("running");
                let stats0 = self.output.as_ref().map(stats).unwrap_or_default();
                self.rec = Some(Rec {
                    id: req.id.to_string(),
                    path: path.clone(),
                    stats0,
                    preroll_ms,
                    segment: Some(seg),
                });
                return Ok(StartOutcome {
                    encoder: run.attempt.encoder.clone(),
                    capture,
                    fallback_reason,
                    preroll_ms,
                    encoder_fallback: fallback_message(&run.first, &run.attempt, &run.failed),
                    memory: self.memory.clone(),
                    notices,
                });
            }
        }

        // Without the buffer: start the output on the match file.
        if self.output_active() {
            let _ = self.stop_output();
            self.run = None;
        }
        notices.extend(self.launch_plan(Some(&path)).map_err(failed)?);
        let run = self.run.as_ref().expect("launched");
        let stats0 = self.output.as_ref().map(stats).unwrap_or_default();
        self.rec = Some(Rec {
            id: req.id.to_string(),
            path,
            stats0,
            preroll_ms: 0,
            segment: None,
        });
        Ok(StartOutcome {
            encoder: run.attempt.encoder.clone(),
            capture,
            fallback_reason,
            preroll_ms: 0,
            encoder_fallback: fallback_message(&run.first, &run.attempt, &run.failed),
            memory: self.memory.clone(),
            notices,
        })
    }

    fn health(&self, rec: &Rec) -> Option<RecordingHealth> {
        let run = self.run.as_ref()?;
        let now = self.output.as_ref().map(stats)?;
        let d = |a: u64, b: u64| a.saturating_sub(b);
        Some(RecordingHealth {
            total_frames: d(now.total, rec.stats0.total),
            dropped_frames: d(now.dropped, rec.stats0.dropped) + d(now.skipped, rec.stats0.skipped),
            lagged_frames: d(now.lagged, rec.stats0.lagged),
            encoder: run.attempt.encoder.clone(),
            encoder_fallback: fallback_message(&run.first, &run.attempt, &run.failed),
            preroll_ms: rec.preroll_ms,
        })
    }

    fn stop_inner(&mut self) -> anyhow::Result<StopOutcome> {
        let Some(rec) = self.rec.take() else {
            return Ok(StopOutcome::default());
        };
        let health = self.health(&rec);
        let mut notices = Vec::new();
        let result = match rec.segment.clone() {
            Some(seg) => {
                // Split off the recording, then move it to the match path.
                let split = self.buffering() && self.rotate() && matches!(self.wait_split(), Ok(Some(_)));
                if !split {
                    log("buffer split failed at stop; stopping the output");
                    let _ = self.stop_output();
                    self.run = None;
                }
                let moved = prebuffer::move_segment(&seg, &rec.path).map_err(|e| anyhow::anyhow!("saving {}: {e}", rec.path.display()));
                prebuffer::remove_marker(&seg);
                if !split {
                    notices.extend(self.start_buffer());
                }
                moved
            }
            None => {
                let r = self.stop_output();
                self.run = None;
                r
            }
        };
        let temporary = self.arm.as_ref().is_some_and(|a| a.temporary);
        if temporary {
            self.disarm_inner();
            if let Some(s) = self.saved.clone() {
                // Back to the real arm (and its buffer).
                let mem = self.memory.clone();
                self.arm_with(&s, mem.as_ref(), false);
                notices.extend(self.settle_arm());
            }
        } else if self.arm.is_some() && !self.buffering() {
            // Direct recording finished: the buffer (if wanted) runs again.
            notices.extend(self.start_buffer());
        }
        notices.extend(self.armed_event());
        result.map(|()| StopOutcome { health, notices })
    }
}

/// A fresh id for a buffer run (segment names never collide with an earlier run's).
fn session_run_id() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Sleep for `total`, checking `abort` every 100 ms. Returns true if aborted.
fn sleep_abortable(total: Duration, abort: &mut dyn FnMut() -> bool) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if abort() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    abort()
}

impl Backend for ObsBackend {
    fn arm(&mut self, req: &ArmRequest) -> Vec<RecorderEvent> {
        let setup = SavedArm {
            capture: req.capture.clone(),
            video: req.video.clone(),
            audio: req.audio.clone(),
            buffer_dir: req.buffer_dir.map(str::to_string),
            preroll_secs: req.preroll_secs,
        };
        self.saved = Some(setup.clone());
        self.arm_with(&setup, req.memory, false);
        self.settle_arm()
    }

    fn disarm(&mut self) {
        self.saved = None;
        if self.rec.is_none() {
            self.disarm_inner();
        }
    }

    fn start(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError> {
        let r = self.start_inner(req, abort);
        if r.is_err() {
            self.rec = None;
            if self.arm.as_ref().is_some_and(|a| a.temporary) {
                self.disarm_inner();
                if let Some(s) = self.saved.clone() {
                    let mem = self.memory.clone();
                    self.arm_with(&s, mem.as_ref(), false);
                }
            }
        }
        r
    }

    fn stop(&mut self) -> anyhow::Result<StopOutcome> {
        self.stop_inner()
    }

    fn tick(&mut self, _recording: bool) -> Vec<RecorderEvent> {
        let mut out = Vec::new();
        let due = |a: &ArmState| a.retry_at.is_none_or(|t| Instant::now() >= t);
        if self.arm.as_ref().is_some_and(|a| a.attached.is_none() && due(a)) && self.rec.is_none() {
            out.extend(self.settle_arm());
        }
        out.extend(self.watch_capture());
        out.extend(self.watch_encoder());
        out.extend(self.tick_buffer());
        out.extend(self.watch_monitor());
        out
    }

    fn encoders(&mut self) -> anyhow::Result<Vec<String>> {
        let list = self.ctx.available_video_encoders().map_err(err("listing encoders"))?;
        Ok(list
            .iter()
            .map(|b| ObsString::from(b.get_encoder_id().clone()).to_string())
            .collect())
    }
}

#[cfg(windows)]
mod capture {
    use super::*;
    use crate::select::{pick_window, MonitorCandidate, WindowCandidate};
    use libobs_simple::sources::windows::{
        game_capture::ObsGameCaptureMode, GameCaptureSourceBuilder, MonitorCaptureSourceBuilder, ObsWindowPriority,
        WindowCaptureSourceBuilder, WindowInfo, WindowSearchMode,
    };
    use libobs_wrapper::{scenes::SceneItemTrait, sources::ObsSourceBuilder, unsafe_send::Sendable};

    pub type Window = Sendable<WindowInfo>;

    pub fn find_window(process: Option<&str>, pid: Option<u32>) -> Option<Window> {
        let windows = WindowCaptureSourceBuilder::get_windows(WindowSearchMode::ExcludeMinimized).ok()?;
        let cands: Vec<WindowCandidate> = windows
            .iter()
            .map(|w| WindowCandidate {
                full_exe: w.0.full_exe.clone(),
                pid: w.0.pid,
                is_game: w.0.is_game,
                has_title: w.0.title.as_deref().is_some_and(|t| !t.trim().is_empty()),
            })
            .collect();
        let idx = pick_window(&cands, process, pid)?;
        windows.into_iter().nth(idx)
    }

    pub fn window_monitor(w: &Window) -> Option<String> {
        w.0.monitor.clone()
    }

    pub fn monitors() -> Vec<MonitorCandidate> {
        MonitorCaptureSourceBuilder::get_monitors()
            .map(|list| {
                list.iter()
                    .map(|m| MonitorCandidate {
                        name: m.0.name.clone(),
                        width: m.0.width,
                        height: m.0.height,
                        primary: m.0.is_primary,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn add_monitor(b: &mut ObsBackend, index: Option<usize>) -> Result<(), String> {
        let monitors = MonitorCaptureSourceBuilder::get_monitors().map_err(|e| format!("listing monitors: {e}"))?;
        let monitor = index
            .and_then(|i| monitors.get(i))
            .or_else(|| monitors.iter().find(|m| m.0.is_primary))
            .or_else(|| monitors.first())
            .ok_or_else(|| "no monitor found".to_string())?;
        let item = b
            .ctx
            .source_builder::<MonitorCaptureSourceBuilder, _>("recap_monitor")
            .map_err(|e| format!("monitor capture: {e}"))?
            .set_monitor(monitor)
            .add_to_scene(&mut b.scene)
            .map_err(|e| format!("monitor capture: {e}"))?;
        let _ = item.fit_source_to_screen();
        Ok(())
    }

    pub fn add_game(b: &mut ObsBackend, window: &Window) -> Result<(SourcePtr, ObsRuntime), String> {
        let fail = |e: libobs_wrapper::utils::ObsError| format!("game capture: {e}");
        let item = b
            .ctx
            .source_builder::<GameCaptureSourceBuilder, _>("recap_game")
            .map_err(fail)?
            .set_capture_mode(ObsGameCaptureMode::CaptureSpecificWindow)
            .set_window(&window.0)
            .set_priority(ObsWindowPriority::Executable)
            .set_anti_cheat_hook(true)
            .set_capture_cursor(false)
            .add_to_scene(&mut b.scene)
            .map_err(fail)?;
        let _ = item.fit_source_to_screen();
        let src = item.inner_source();
        Ok((src.as_ptr(), src.runtime().clone()))
    }

    pub fn add_window(b: &mut ObsBackend, window: &Window) -> Result<(), String> {
        let fail = |e: libobs_wrapper::utils::ObsError| format!("window capture: {e}");
        let item = b
            .ctx
            .source_builder::<WindowCaptureSourceBuilder, _>("recap_window")
            .map_err(fail)?
            .set_window(window)
            .set_priority(ObsWindowPriority::Executable)
            .set_cursor(false)
            .add_to_scene(&mut b.scene)
            .map_err(fail)?;
        let _ = item.fit_source_to_screen();
        Ok(())
    }
}

/// Non-Windows builds (linux-typecheck only) cannot capture anything.
#[cfg(not(windows))]
mod capture {
    use super::*;
    use crate::select::MonitorCandidate;

    pub struct Window;

    pub fn find_window(_process: Option<&str>, _pid: Option<u32>) -> Option<Window> {
        None
    }

    pub fn window_monitor(_w: &Window) -> Option<String> {
        None
    }

    pub fn monitors() -> Vec<MonitorCandidate> {
        vec![]
    }

    pub fn add_monitor(_b: &mut ObsBackend, _index: Option<usize>) -> Result<(), String> {
        Err("capture is only implemented on Windows".into())
    }

    pub fn add_game(_b: &mut ObsBackend, _w: &Window) -> Result<(SourcePtr, ObsRuntime), String> {
        Err("capture is only implemented on Windows".into())
    }

    pub fn add_window(_b: &mut ObsBackend, _w: &Window) -> Result<(), String> {
        Err("capture is only implemented on Windows".into())
    }
}
