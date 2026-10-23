//! Command loop: turns protocol commands into backend calls and protocol events.
//! Backend-agnostic so it can be tested without OBS.

use std::{
    collections::VecDeque,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use recap_core::{
    games::CaptureMode,
    protocol::{AudioConfig, CaptureTarget, RecorderCommand, RecorderEvent, VideoConfig},
};

/// What the stdin reader thread delivers.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Command(RecorderCommand),
    /// A line that is not a valid command.
    Invalid(String),
    /// stdin closed: the app is gone.
    Eof,
}

pub struct StartRequest<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub capture: &'a CaptureTarget,
    pub video: &'a VideoConfig,
    pub audio: &'a AudioConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartOutcome {
    /// OBS encoder id actually used.
    pub encoder: String,
    /// Capture mode actually used.
    pub capture: CaptureMode,
    /// Set when the capture fell back from the requested mode.
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    /// `abort()` returned true while waiting (Stop/Shutdown arrived).
    Aborted,
    Failed(String),
}

pub trait Backend {
    /// Start recording. Long waits (e.g. for the game window) must call `abort()`
    /// regularly and return `StartError::Aborted` when it returns true.
    fn start(&mut self, req: &StartRequest, abort: &mut dyn FnMut() -> bool) -> Result<StartOutcome, StartError>;
    /// Stop the current recording and finalize the file.
    fn stop(&mut self) -> anyhow::Result<()>;
    fn encoders(&mut self) -> anyhow::Result<Vec<String>>;
}

struct Active {
    id: String,
    path: String,
    since: Instant,
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs until Shutdown or EOF. Every protocol event goes through `emit`.
pub fn run<B: Backend>(backend: &mut B, rx: Receiver<Incoming>, emit: &mut dyn FnMut(RecorderEvent)) {
    let mut active: Option<Active> = None;
    // Commands received while a start was waiting, handled afterwards.
    let mut pending: VecDeque<Incoming> = VecDeque::new();

    loop {
        let msg = match pending.pop_front() {
            Some(m) => m,
            None => rx.recv().unwrap_or(Incoming::Eof),
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
                return;
            }
            Incoming::Command(RecorderCommand::ListEncoders) => match backend.encoders() {
                Ok(encoders) => emit(RecorderEvent::Encoders { encoders }),
                Err(e) => emit(RecorderEvent::Error {
                    id: None,
                    message: format!("listing encoders failed: {e:#}"),
                }),
            },
            Incoming::Command(RecorderCommand::Stop { id }) => match active.take() {
                Some(a) if a.id == id => finish(backend, a, emit),
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
                };
                let result = backend.start(&req, &mut abort);
                match result {
                    Ok(out) => {
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
                            started_unix_ms: unix_ms(),
                            encoder: out.encoder,
                            capture: out.capture,
                        });
                        active = Some(Active {
                            id,
                            path,
                            since: Instant::now(),
                        });
                        // A stop/shutdown that raced with a successful start is honoured now.
                        if let Some(k) = abort_kind {
                            pending.push_front(k);
                        }
                    }
                    Err(StartError::Aborted) => match abort_kind {
                        Some(Incoming::Command(RecorderCommand::Stop { .. })) | None => {
                            emit(RecorderEvent::Stopped { id, path, duration_ms: 0 })
                        }
                        Some(_) => return,
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
    let duration_ms = a.since.elapsed().as_millis() as u64;
    match backend.stop() {
        Ok(()) => emit(RecorderEvent::Stopped {
            id: a.id,
            path: a.path,
            duration_ms,
        }),
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
    }

    impl Backend for Fake {
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
                fallback_reason: Some("no window".into()),
            })
        }
        fn stop(&mut self) -> anyhow::Result<()> {
            self.log.push("stop".into());
            Ok(())
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

    #[test]
    fn blank_lines_are_ignored() {
        assert_eq!(parse_line("   "), None);
        assert!(matches!(parse_line("{}"), Some(Incoming::Invalid(_))));
    }
}
