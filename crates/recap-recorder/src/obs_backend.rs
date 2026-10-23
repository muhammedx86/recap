//! The real recorder: a libobs context with one scene, one capture source, optional
//! desktop/mic audio and one MP4 output that is reused between recordings.
//!
//! libobs-wrapper marshals every OBS call onto its own runtime thread
//! (`enable_runtime`), so this type can be driven from the command loop thread.

use std::{
    fmt::Display,
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use libobs_wrapper::{
    context::ObsContext,
    data::{
        output::{ObsOutputRef, ObsOutputTrait},
        video::ObsVideoInfoBuilder,
        ObsDataSetters,
    },
    encoders::{ObsAudioEncoderType, ObsContextEncoders, ObsVideoEncoderType},
    enums::ObsLogLevel,
    logger::ObsLogger,
    scenes::{ObsSceneRef, SceneItemExtSceneTrait},
    utils::{AudioEncoderInfo, ObsString, OutputInfo, SourceInfo, StartupInfo, VideoEncoderInfo},
};
use recap_core::{games::CaptureMode, protocol::VideoConfig};

use crate::{
    select::{choose_encoder, is_hardware, output_size, X264},
    session::{Backend, StartError, StartOutcome, StartRequest},
};

/// How long to wait for the game window to appear before falling back to monitor capture.
const WINDOW_WAIT: Duration = Duration::from_secs(60);
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

pub struct ObsBackend {
    ctx: ObsContext,
    scene: ObsSceneRef,
    output: Option<ObsOutputRef>,
    /// (fps, output width, output height) currently applied.
    video: Option<(u32, u32, u32)>,
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
            },
            version,
        ))
    }

    /// Apply fps/resolution. The canvas (base) is the primary monitor; output may be scaled down.
    fn configure_video(&mut self, v: &VideoConfig) -> anyhow::Result<()> {
        let probe = ObsVideoInfoBuilder::new().build();
        let (bw, bh) = (probe.get_base_width(), probe.get_base_height());
        let (ow, oh) = output_size(bw, bh, v.output_height);
        let fps = v.fps.clamp(10, 240);
        if self.video == Some((fps, ow, oh)) {
            return Ok(());
        }
        let ovi = ObsVideoInfoBuilder::new()
            .fps_num(fps)
            .fps_den(1)
            .base_width(bw)
            .base_height(bh)
            .output_width(ow)
            .output_height(oh)
            .build();
        self.ctx.reset_video(ovi).map_err(err("applying video settings"))?;
        self.video = Some((fps, ow, oh));
        Ok(())
    }

    /// Add audio sources requested by the app to the scene.
    fn add_audio(&mut self, id: &str, desktop: bool, mic: bool, mic_device: Option<&str>) -> anyhow::Result<()> {
        if desktop {
            let mut data = self.ctx.data().map_err(err("audio settings"))?;
            data.set_string("device_id", "default").map_err(err("audio settings"))?;
            self.scene
                .add_and_create_source(SourceInfo::new(
                    "wasapi_output_capture",
                    format!("recap_desktop_{id}"),
                    Some(data),
                    None,
                ))
                .map_err(err("adding desktop audio"))?;
        }
        if mic {
            let mut data = self.ctx.data().map_err(err("mic settings"))?;
            data.set_string("device_id", mic_device.unwrap_or("default"))
                .map_err(err("mic settings"))?;
            self.scene
                .add_and_create_source(SourceInfo::new("wasapi_input_capture", format!("recap_mic_{id}"), Some(data), None))
                .map_err(err("adding microphone"))?;
        }
        Ok(())
    }

    /// Configure the (reused) output for `path` with `encoder` and start it.
    fn start_output(&mut self, path: &str, encoder: &str, bitrate_kbps: u32) -> anyhow::Result<()> {
        let mut out_settings = self.ctx.data().map_err(err("output settings"))?;
        out_settings.set_string("path", path).map_err(err("output settings"))?;
        if self.output.is_none() {
            // mp4_output is OBS's hybrid MP4 muxer: the file stays playable if we crash.
            let output = self
                .ctx
                .output(OutputInfo::new("mp4_output", OUTPUT_NAME, Some(out_settings), None))
                .map_err(err("creating output"))?;
            self.output = Some(output);
        } else {
            self.ctx.update_output(OUTPUT_NAME, out_settings).map_err(err("updating output"))?;
        }

        let mut vs = self.ctx.data().map_err(err("encoder settings"))?;
        vs.set_string("rate_control", "CBR").map_err(err("encoder settings"))?;
        vs.set_int("bitrate", bitrate_kbps.clamp(1000, 150_000) as i64)
            .map_err(err("encoder settings"))?;
        // 1 s keyframes keep stream-copy clip cuts close to the requested time.
        vs.set_int("keyint_sec", 1).map_err(err("encoder settings"))?;
        if encoder == X264 {
            vs.set_string("preset", "veryfast").map_err(err("encoder settings"))?;
            vs.set_string("profile", "high").map_err(err("encoder settings"))?;
        }
        let mut aud = self.ctx.data().map_err(err("audio encoder settings"))?;
        aud.set_int("bitrate", 160).map_err(err("audio encoder settings"))?;

        let output = self.output.as_mut().expect("output created above");
        let enc_type = ObsVideoEncoderType::from_str(encoder).expect("infallible");
        output
            .create_and_set_video_encoder(VideoEncoderInfo::new(enc_type, "recap_video_encoder", Some(vs), None))
            .map_err(err("creating video encoder"))?;
        output
            .create_and_set_audio_encoder(
                AudioEncoderInfo::new(ObsAudioEncoderType::FFMPEG_AAC, "recap_audio_encoder", Some(aud), None),
                0,
            )
            .map_err(err("creating audio encoder"))?;
        output.start().map_err(err("starting output"))?;
        Ok(())
    }

    fn clear_scene(&mut self) {
        if let Err(e) = self.scene.remove_all_sources() {
            eprintln!("[recorder] clearing scene failed: {e}");
        }
    }

    fn start_inner(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError> {
        let failed = |e: anyhow::Error| StartError::Failed(format!("{e:#}"));
        self.configure_video(req.video).map_err(failed)?;

        let (capture, fallback_reason) = capture::add_capture(self, req, abort)?;
        self.add_audio(
            req.id,
            req.audio.desktop,
            req.audio.microphone,
            req.audio.microphone_device.as_deref(),
        )
        .map_err(failed)?;

        let available = self.encoders().unwrap_or_default();
        let encoder = choose_encoder(&req.video.encoder, &available);
        match self.start_output(req.path, &encoder, req.video.bitrate_kbps) {
            Ok(()) => Ok(StartOutcome {
                encoder,
                capture,
                fallback_reason,
            }),
            Err(e) if is_hardware(&encoder) => {
                eprintln!("[recorder] {encoder} failed ({e:#}); retrying with x264");
                self.start_output(req.path, X264, req.video.bitrate_kbps).map_err(failed)?;
                Ok(StartOutcome {
                    encoder: X264.to_string(),
                    capture,
                    fallback_reason,
                })
            }
            Err(e) => Err(failed(e)),
        }
    }
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
    fn start(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError> {
        let r = self.start_inner(req, abort);
        if r.is_err() {
            self.clear_scene();
        }
        r
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        let result = match self.output.as_mut() {
            Some(o) => o.stop().map_err(err("stopping output")),
            None => Ok(()),
        };
        self.clear_scene();
        result
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
    use crate::select::{pick_window, WindowCandidate};
    use libobs_simple::sources::windows::{
        game_capture::ObsGameCaptureMode, GameCaptureSourceBuilder, MonitorCaptureSourceBuilder, ObsWindowPriority,
        WindowCaptureSourceBuilder, WindowInfo, WindowSearchMode,
    };
    use libobs_wrapper::{scenes::SceneItemTrait, sources::ObsSourceBuilder, unsafe_send::Sendable};

    fn find_window(process: Option<&str>, pid: Option<u32>) -> Option<Sendable<WindowInfo>> {
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

    fn add_monitor(b: &mut ObsBackend, id: &str, index: Option<u32>) -> Result<(), StartError> {
        let monitors = MonitorCaptureSourceBuilder::get_monitors().map_err(|e| StartError::Failed(format!("listing monitors: {e}")))?;
        let monitor = index
            .and_then(|i| monitors.get(i as usize))
            .or_else(|| monitors.iter().find(|m| m.0.is_primary))
            .or_else(|| monitors.first())
            .ok_or_else(|| StartError::Failed("no monitor found".into()))?;
        let item = b
            .ctx
            .source_builder::<MonitorCaptureSourceBuilder, _>(format!("recap_monitor_{id}"))
            .map_err(|e| StartError::Failed(format!("monitor capture: {e}")))?
            .set_monitor(monitor)
            .add_to_scene(&mut b.scene)
            .map_err(|e| StartError::Failed(format!("monitor capture: {e}")))?;
        let _ = item.fit_source_to_screen();
        Ok(())
    }

    /// Adds the capture source for the request. Returns the capture mode used and a
    /// fallback reason when it differs from the requested one.
    pub(super) fn add_capture(
        b: &mut ObsBackend,
        req: &StartRequest,
        abort: &mut dyn FnMut() -> bool,
    ) -> Result<(CaptureMode, Option<String>), StartError> {
        let target = req.capture;
        if target.mode == CaptureMode::Monitor {
            add_monitor(b, req.id, target.monitor)?;
            return Ok((CaptureMode::Monitor, None));
        }

        // Game and window modes need the game's window; wait while the game boots.
        let deadline = Instant::now() + WINDOW_WAIT;
        let window = loop {
            if let Some(w) = find_window(target.process.as_deref(), target.pid) {
                break Some(w);
            }
            if Instant::now() >= deadline {
                break None;
            }
            if sleep_abortable(Duration::from_millis(500), abort) {
                return Err(StartError::Aborted);
            }
        };
        let Some(window) = window else {
            add_monitor(b, req.id, target.monitor)?;
            let what = target
                .process
                .clone()
                .or(target.pid.map(|p| format!("pid {p}")))
                .unwrap_or_else(|| "the game".into());
            return Ok((
                CaptureMode::Monitor,
                Some(format!(
                    "no window for {what} after {}s; recording the monitor",
                    WINDOW_WAIT.as_secs()
                )),
            ));
        };

        let fail = |e: libobs_wrapper::utils::ObsError| StartError::Failed(format!("capture source: {e}"));
        match target.mode {
            CaptureMode::Game => {
                let item = b
                    .ctx
                    .source_builder::<GameCaptureSourceBuilder, _>(format!("recap_game_{}", req.id))
                    .map_err(fail)?
                    .set_capture_mode(ObsGameCaptureMode::CaptureSpecificWindow)
                    .set_window(&window.0)
                    .set_priority(ObsWindowPriority::Executable)
                    .set_anti_cheat_hook(true)
                    .set_capture_cursor(false)
                    .add_to_scene(&mut b.scene)
                    .map_err(fail)?;
                let _ = item.fit_source_to_screen();
                Ok((CaptureMode::Game, None))
            }
            _ => {
                let item = b
                    .ctx
                    .source_builder::<WindowCaptureSourceBuilder, _>(format!("recap_window_{}", req.id))
                    .map_err(fail)?
                    .set_window(&window)
                    .set_priority(ObsWindowPriority::Executable)
                    .set_cursor(false)
                    .add_to_scene(&mut b.scene)
                    .map_err(fail)?;
                let _ = item.fit_source_to_screen();
                Ok((CaptureMode::Window, None))
            }
        }
    }
}

/// Non-Windows builds (linux-typecheck only) cannot capture anything.
#[cfg(not(windows))]
mod capture {
    use super::*;

    pub(super) fn add_capture(
        _b: &mut ObsBackend,
        _req: &StartRequest,
        abort: &mut dyn FnMut() -> bool,
    ) -> Result<(CaptureMode, Option<String>), StartError> {
        if sleep_abortable(Duration::ZERO, abort) {
            return Err(StartError::Aborted);
        }
        Err(StartError::Failed("capture is only implemented on Windows".into()))
    }
}
