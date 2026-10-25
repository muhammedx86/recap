//! Command loop: turns protocol commands into backend calls and protocol events.
//! Backend-agnostic so it can be tested without OBS.

use std::{
    collections::VecDeque,
    sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use recap_core::{
    games::CaptureMode,
    protocol::{AudioConfig, CaptureTarget, EncoderMemory, RecorderCommand, RecorderEvent, RecordingHealth, VideoConfig},
};

/// How often the backend gets a `tick` (buffer rotation, capture and encoder watchdogs) while
/// no command arrives.
const TICK: Duration = Duration::from_millis(250);

/// What the stdin reader thread delivers (one at a time, so the size difference is fine).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Incoming {
    Command(RecorderCommand),
    /// A line that is not a valid command.
    Invalid(String),
    /// stdin closed: the app is gone.
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArmRequest<'a> {
    pub capture: &'a CaptureTarget,
    pub video: &'a VideoConfig,
    pub audio: &'a AudioConfig,
    pub buffer_dir: Option<&'a str>,
    pub preroll_secs: u32,
    pub memory: Option<&'a EncoderMemory>,
}

pub struct StartRequest<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub capture: &'a CaptureTarget,
    pub video: &'a VideoConfig,
    pub audio: &'a AudioConfig,
    pub memory: Option<&'a EncoderMemory>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartOutcome {
    /// OBS encoder id actually used.
    pub encoder: String,
    /// Capture mode actually used.
    pub capture: CaptureMode,
    /// Set when the capture fell back from the requested mode.
    pub fallback_reason: Option<String>,
    /// Footage kept from before the request (pre-roll), in ms.
    pub preroll_ms: u64,
    /// Set when another encoder than the preferred one records.
    pub encoder_fallback: Option<String>,
    pub memory: Option<EncoderMemory>,
    /// Emitted before `Started` (e.g. `Armed`, warnings).
    pub notices: Vec<RecorderEvent>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StopOutcome {
    pub health: Option<RecordingHealth>,
    /// Emitted after `Stopped`.
    pub notices: Vec<RecorderEvent>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    /// `abort()` returned true while waiting (Stop/Shutdown arrived).
    Aborted,
    Failed(String),
}

pub trait Backend {
    /// Prepare capture, audio and encoder for a game session. Must not block for long: a game
    /// window that is not there yet is picked up by `tick`. Returns events to emit now.
    fn arm(&mut self, req: &ArmRequest) -> Vec<RecorderEvent>;
    /// Release everything `arm` set up.
    fn disarm(&mut self);
    /// Start recording. Long waits (e.g. for the game window) must call `abort()`
    /// regularly and return `StartError::Aborted` when it returns true.
    fn start(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError>;
    /// Stop the current recording and finalize the file at the path given to `start`.
    fn stop(&mut self) -> anyhow::Result<StopOutcome>;
    /// Periodic housekeeping; `recording` is true while a recording is active.
    fn tick(&mut self, recording: bool) -> Vec<RecorderEvent>;
    fn encoders(&mut self) -> anyhow::Result<Vec<String>>;
}

struct Active {
    id: String,
    path: String,
    since: Instant,
    preroll_ms: u64,
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn arm_or_disarm<B: Backend>(backend: &mut B, cmd: RecorderCommand, emit: &mut dyn FnMut(RecorderEvent)) {
    match cmd {
        RecorderCommand::Arm {
            capture,
            video,
            audio,
            buffer_dir,
            preroll_secs,
            encoder_memory,
        } => {
            let req = ArmRequest {
                capture: &capture,
                video: &video,
                audio: &audio,
                buffer_dir: buffer_dir.as_deref(),
                preroll_secs,
                memory: encoder_memory.as_ref(),
            };
            for e in backend.arm(&req) {
                emit(e);
            }
        }
        RecorderCommand::Disarm => {
            backend.disarm();
            emit(RecorderEvent::Disarmed);
        }
        _ => {}
    }
}

/// Runs until Shutdown or EOF. Every protocol event goes through `emit`.
pub fn run<B: Backend>(backend: &mut B, rx: Receiver<Incoming>, emit: &mut dyn FnMut(RecorderEvent)) {
    let mut active: Option<Active> = None;
    // Commands received while a start was waiting, handled afterwards.
    let mut pending: VecDeque<Incoming> = VecDeque::new();
    // The latest Arm/Disarm received while recording, applied when it stops.
    let mut deferred: Option<RecorderCommand> = None;

    loop {
        let msg = match pending.pop_front() {
            Some(m) => m,
            None => match rx.recv_timeout(TICK) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    for e in backend.tick(active.is_some()) {
                        emit(e);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => Incoming::Eof,
            },
        };
        match msg {
            Incoming::Invalid(line) => emit(RecorderEvent::Error {
                id: None,
                message: format!("invalid command: {line}"),
            }),
            Incoming::Eof | Incoming::Command(RecorderCommand::Shutdown) => {
                if let Some(a) = active.take() {
                    finish(backend, a, emit);
                }
                backend.disarm();
                return;
            }
            Incoming::Command(c @ (RecorderCommand::Arm { .. } | RecorderCommand::Disarm)) => {
                if active.is_some() {
                    deferred = Some(c);
                } else {
                    arm_or_disarm(backend, c, emit);
                }
            }
            Incoming::Command(RecorderCommand::ListEncoders) => match backend.encoders() {
                Ok(encoders) => emit(RecorderEvent::Encoders { encoders }),
                Err(e) => emit(RecorderEvent::Error {
                    id: None,
                    message: format!("listing encoders failed: {e:#}"),
                }),
            },
            Incoming::Command(RecorderCommand::Stop { id }) => match active.take() {
                Some(a) if a.id == id => {
                    finish(backend, a, emit);
                    if let Some(c) = deferred.take() {
                        arm_or_disarm(backend, c, emit);
                    }
                }
                Some(a) => {
                    let msg = format!("stop for {id} ignored: recording {} is active", a.id);
                    active = Some(a);
                    emit(RecorderEvent::Error {
                        id: Some(id),
                        message: msg,
                    });
                }
                None => emit(RecorderEvent::Error {
                    id: Some(id),
                    message: "not recording".into(),
                }),
            },
            Incoming::Command(RecorderCommand::Start {
                id,
                path,
                capture,
                video,
                audio,
                encoder_memory,
            }) => {
                if let Some(a) = &active {
                    emit(RecorderEvent::Error {
                        id: Some(id),
                        message: format!("already recording {}", a.id),
                    });
                    continue;
                }
                // While starting, watch stdin for a Stop of this id, Shutdown or EOF.
                let mut abort_kind: Option<Incoming> = None;
                let mut abort = || {
                    if abort_kind.is_some() {
                        return true;
                    }
                    loop {
                        match rx.try_recv() {
                            Ok(Incoming::Command(RecorderCommand::Stop { id: sid })) if sid == id => {
                                abort_kind = Some(Incoming::Command(RecorderCommand::Stop { id: sid }));
                                return true;
                            }
                            Ok(m @ (Incoming::Eof | Incoming::Command(RecorderCommand::Shutdown))) => {
                                abort_kind = Some(m);
                                return true;
                            }
                            Ok(other) => pending.push_back(other),
                            Err(TryRecvError::Empty) => return false,
                            Err(TryRecvError::Disconnected) => {
                                abort_kind = Some(Incoming::Eof);
                                return true;
                            }
                        }
                    }
                };
                let req = StartRequest {
                    id: &id,
                    path: &path,
                    capture: &capture,
                    video: &video,
                    audio: &audio,
                    memory: encoder_memory.as_ref(),
                };
                let result = backend.start(&req, &mut abort);
                match result {
                    Ok(out) => {
                        for e in out.notices {
                            emit(e);
                        }
                        if let Some(reason) = out.fallback_reason {
                            emit(RecorderEvent::CaptureChanged {
                                id: id.clone(),
                                capture: out.capture,
                                reason,
                            });
                        }
                        emit(RecorderEvent::Started {
                            id: id.clone(),
                            path: path.clone(),
                            started_unix_ms: unix_ms().saturating_sub(out.preroll_ms),
                            encoder: out.encoder,
                            capture: out.capture,
                            preroll_ms: out.preroll_ms,
                            encoder_fallback: out.encoder_fallback,
                            encoder_memory: out.memory,
                        });
                        active = Some(Active {
                            id,
                            path,
                            since: Instant::now(),
                            preroll_ms: out.preroll_ms,
                        });
                        // A stop/shutdown that raced with a successful start is honoured now.
                        if let Some(k) = abort_kind {
                            pending.push_front(k);
                        }
                    }
                    Err(StartError::Aborted) => match abort_kind {
                        Some(Incoming::Command(RecorderCommand::Stop { .. })) | None => emit(RecorderEvent::Stopped {
                            id,
                            path,
                            duration_ms: 0,
                            health: None,
                        }),
                        Some(_) => {
                            backend.disarm();
                            return;
                        }
                    },
                    Err(StartError::Failed(message)) => {
                        emit(RecorderEvent::Error { id: Some(id), message });
                        // A Stop for the failed start needs no answer beyond the error;
                        // Shutdown/EOF must still end the loop.
                        if let Some(k @ (Incoming::Eof | Incoming::Command(RecorderCommand::Shutdown))) = abort_kind {
                            pending.push_front(k);
                        }
                    }
                }
            }
        }
    }
}

fn finish<B: Backend>(backend: &mut B, a: Active, emit: &mut dyn FnMut(RecorderEvent)) {
    let duration_ms = a.since.elapsed().as_millis() as u64 + a.preroll_ms;
    match backend.stop() {
        Ok(out) => {
            emit(RecorderEvent::Stopped {
                id: a.id,
                path: a.path,
                duration_ms,
                health: out.health,
            });
            for e in out.notices {
                emit(e);
            }
        }
        Err(e) => {
            emit(RecorderEvent::Error {
                id: Some(a.id.clone()),
                message: format!("stopping failed: {e:#}"),
            });
            // Still report the stop so the app does not wait forever; the file may be partial.
            emit(RecorderEvent::Stopped {
                id: a.id,
                path: a.path,
                duration_ms,
                health: None,
            });
        }
    }
}

/// Parse one stdin line.
pub fn parse_line(line: &str) -> Option<Incoming> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    Some(match serde_json::from_str::<RecorderCommand>(line) {
        Ok(c) => Incoming::Command(c),
        Err(e) => Incoming::Invalid(format!("{line} ({e})")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        /// Number of abort() polls before start succeeds; None = succeed immediately.
        wait_polls: Option<u32>,
        fail_start: bool,
        armed: bool,
        ticks: u32,
    }

    impl Backend for Fake {
        fn arm(&mut self, req: &ArmRequest) -> Vec<RecorderEvent> {
            self.log.push(format!("arm {}", req.preroll_secs));
            self.armed = true;
            vec![RecorderEvent::Armed {
                capture: req.capture.mode,
                encoder: Some("obs_x264".into()),
                buffering: req.preroll_secs > 0,
                encoder_fallback: None,
                encoder_memory: None,
            }]
        }
        fn disarm(&mut self) {
            if self.armed {
                self.log.push("disarm".into());
            }
            self.armed = false;
        }
        fn tick(&mut self, _recording: bool) -> Vec<RecorderEvent> {
            self.ticks += 1;
            vec![]
        }
        fn start(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError> {
            self.log.push(format!("start {}", req.id));
            if let Some(n) = self.wait_polls {
                for _ in 0..n {
                    if abort() {
                        self.log.push("aborted".into());
                        return Err(StartError::Aborted);
                    }
                }
            }
            if self.fail_start {
                return Err(StartError::Failed("boom".into()));
            }
            Ok(StartOutcome {
                encoder: "obs_x264".into(),
                capture: CaptureMode::Monitor,
                fallback_reason: (!self.armed).then(|| "no window".into()),
                preroll_ms: if self.armed { 4_000 } else { 0 },
                ..Default::default()
            })
        }
        fn stop(&mut self) -> anyhow::Result<StopOutcome> {
            self.log.push("stop".into());
            Ok(StopOutcome {
                health: Some(RecordingHealth {
                    total_frames: 100,
                    encoder: "obs_x264".into(),
                    ..Default::default()
                }),
                notices: vec![],
            })
        }
        fn encoders(&mut self) -> anyhow::Result<Vec<String>> {
            Ok(vec!["obs_x264".into()])
        }
    }

    fn start_cmd(id: &str) -> Incoming {
        parse_line(&format!(
            r#"{{"cmd":"start","id":"{id}","path":"C:/r/{id}.mp4","capture":{{"mode":"game","process":"x.exe"}},"video":{{"fps":60,"bitrateKbps":8000,"encoder":"auto"}},"audio":{{"desktop":true,"microphone":false}}}}"#
        ))
        .unwrap()
    }

    fn drive(fake: &mut Fake, msgs: Vec<Incoming>) -> Vec<RecorderEvent> {
        let (tx, rx) = channel();
        for m in msgs {
            tx.send(m).unwrap();
        }
        drop(tx);
        let mut out = Vec::new();
        run(fake, rx, &mut |e| out.push(e));
        out
    }

    #[test]
    fn start_stop_cycle() {
        let mut f = Fake::default();
        let ev = drive(
            &mut f,
            vec![
                start_cmd("a"),
                start_cmd("b"),
                parse_line(r#"{"cmd":"listEncoders"}"#).unwrap(),
                parse_line(r#"{"cmd":"stop","id":"zzz"}"#).unwrap(),
                parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap(),
                parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap(),
                parse_line("garbage").unwrap(),
                parse_line(r#"{"cmd":"shutdown"}"#).unwrap(),
            ],
        );
        let kinds: Vec<String> = ev
            .iter()
            .map(|e| serde_json::to_value(e).unwrap()["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            kinds,
            [
                "captureChanged",
                "started",
                "error",
                "encoders",
                "error",
                "stopped",
                "error",
                "error"
            ]
        );
        assert_eq!(f.log, ["start a", "stop"]);
        assert!(matches!(&ev[5], RecorderEvent::Stopped { health: Some(h), .. } if h.total_frames == 100));
        match &ev[1] {
            RecorderEvent::Started { id, encoder, capture, .. } => {
                assert_eq!(id, "a");
                assert_eq!(encoder, "obs_x264");
                assert_eq!(*capture, CaptureMode::Monitor);
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn eof_finalizes_active_recording() {
        let mut f = Fake::default();
        let ev = drive(&mut f, vec![start_cmd("a")]);
        assert_eq!(f.log, ["start a", "stop"]);
        assert!(matches!(ev.last(), Some(RecorderEvent::Stopped { id, .. }) if id == "a"));
    }

    #[test]
    fn stop_during_window_wait_aborts_start() {
        let mut f = Fake {
            wait_polls: Some(5),
            ..Default::default()
        };
        let ev = drive(
            &mut f,
            vec![
                start_cmd("a"),
                parse_line(r#"{"cmd":"listEncoders"}"#).unwrap(),
                parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap(),
            ],
        );
        assert_eq!(f.log, ["start a", "aborted"]);
        // The stop is answered with a zero-length Stopped, the queued listEncoders still runs.
        assert!(matches!(&ev[0], RecorderEvent::Stopped { id, duration_ms: 0, .. } if id == "a"));
        assert!(matches!(&ev[1], RecorderEvent::Encoders { .. }));
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn shutdown_during_wait_exits_quietly() {
        let mut f = Fake {
            wait_polls: Some(5),
            ..Default::default()
        };
        let ev = drive(&mut f, vec![start_cmd("a"), parse_line(r#"{"cmd":"shutdown"}"#).unwrap()]);
        assert_eq!(f.log, ["start a", "aborted"]);
        assert!(ev.is_empty());
    }

    #[test]
    fn failed_start_reports_error() {
        let mut f = Fake {
            fail_start: true,
            ..Default::default()
        };
        let ev = drive(&mut f, vec![start_cmd("a"), parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap()]);
        assert!(matches!(&ev[0], RecorderEvent::Error { id: Some(id), message } if id == "a" && message == "boom"));
        assert!(matches!(&ev[1], RecorderEvent::Error { message, .. } if message == "not recording"));
    }

    const ARM: &str = r#"{"cmd":"arm","capture":{"mode":"game","process":"x.exe"},"video":{"fps":60,"bitrateKbps":8000,"encoder":"auto"},"audio":{"desktop":true,"microphone":false},"bufferDir":"C:/r/.recap-buffer","prerollSecs":10}"#;

    #[test]
    fn armed_start_reports_preroll_and_backdates_start() {
        let mut f = Fake::default();
        let before = unix_ms();
        let ev = drive(
            &mut f,
            vec![
                parse_line(ARM).unwrap(),
                start_cmd("a"),
                parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap(),
                parse_line(r#"{"cmd":"disarm"}"#).unwrap(),
            ],
        );
        assert_eq!(f.log, ["arm 10", "start a", "stop", "disarm"]);
        assert!(matches!(&ev[0], RecorderEvent::Armed { buffering: true, .. }));
        match &ev[1] {
            RecorderEvent::Started {
                preroll_ms,
                started_unix_ms,
                ..
            } => {
                assert_eq!(*preroll_ms, 4_000);
                assert!(
                    *started_unix_ms <= before.saturating_sub(3_900),
                    "start is back-dated by the pre-roll"
                );
            }
            e => panic!("{e:?}"),
        }
        // Duration includes the pre-roll.
        assert!(matches!(&ev[2], RecorderEvent::Stopped { duration_ms, .. } if *duration_ms >= 4_000));
        assert!(matches!(&ev[3], RecorderEvent::Disarmed));
    }

    #[test]
    fn arm_while_recording_waits_for_the_stop() {
        let mut f = Fake::default();
        let ev = drive(
            &mut f,
            vec![
                start_cmd("a"),
                parse_line(ARM).unwrap(),
                parse_line(r#"{"cmd":"stop","id":"a"}"#).unwrap(),
            ],
        );
        // The arm runs after the stop; EOF then disarms.
        assert_eq!(f.log, ["start a", "stop", "arm 10", "disarm"]);
        let kinds: Vec<String> = ev
            .iter()
            .map(|e| serde_json::to_value(e).unwrap()["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["captureChanged", "started", "stopped", "armed"]);
    }

    #[test]
    fn idle_loop_ticks_the_backend() {
        let mut f = Fake::default();
        let (tx, rx) = channel();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            tx.send(parse_line(r#"{"cmd":"shutdown"}"#).unwrap()).unwrap();
        });
        run(&mut f, rx, &mut |_| {});
        h.join().unwrap();
        assert!(f.ticks >= 2, "ticks: {}", f.ticks);
    }

    #[test]
    fn blank_lines_are_ignored() {
        assert_eq!(parse_line("   "), None);
        assert!(matches!(parse_line("{}"), Some(Incoming::Invalid(_))));
    }
}
