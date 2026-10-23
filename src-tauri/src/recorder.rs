//! Owns the `recap-recorder` sidecar process and its JSON-lines pipes.

use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use recap_core::protocol::{RecorderCommand, RecorderEvent};

type Handler = Arc<dyn Fn(RecorderEvent) + Send + Sync>;

struct Proc {
    child: Child,
    stdin: ChildStdin,
}

/// Lazily spawned; respawned if it died.
pub struct Recorder {
    exe: PathBuf,
    log_file: PathBuf,
    proc: Mutex<Option<Proc>>,
    handler: Handler,
    /// Waiters for an `Encoders` reply.
    encoder_waiters: Arc<Mutex<Vec<mpsc::Sender<Vec<String>>>>>,
}

impl Recorder {
    /// `handler` receives every event except `Encoders` replies (and a synthetic
    /// `Error { id: None }` when the process exits unexpectedly).
    pub fn new(exe: PathBuf, log_file: PathBuf, handler: impl Fn(RecorderEvent) + Send + Sync + 'static) -> Self {
        Self {
            exe,
            log_file,
            proc: Mutex::new(None),
            handler: Arc::new(handler),
            encoder_waiters: Arc::default(),
        }
    }

    pub fn available(&self) -> bool {
        self.exe.exists()
    }

    fn ensure_running<'a>(&self, guard: &'a mut Option<Proc>) -> Result<&'a mut Proc> {
        let alive = match guard.as_mut() {
            Some(p) => p.child.try_wait().ok().flatten().is_none(),
            None => false,
        };
        if !alive {
            *guard = Some(self.spawn()?);
        }
        Ok(guard.as_mut().expect("just spawned"))
    }

    fn spawn(&self) -> Result<Proc> {
        if !self.exe.exists() {
            bail!("recorder not found at {}", self.exe.display());
        }
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null());
        let mut cmd = Command::new(&self.exe);
        if let Some(dir) = self.exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().with_context(|| format!("starting {}", self.exe.display()))?;
        let stdin = child.stdin.take().context("recorder stdin")?;
        let stdout = child.stdout.take().context("recorder stdout")?;
        let handler = self.handler.clone();
        let waiters = self.encoder_waiters.clone();
        std::thread::Builder::new().name("recorder-reader".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<RecorderEvent>(line) {
                    Ok(RecorderEvent::Encoders { encoders }) => {
                        for w in waiters.lock().unwrap().drain(..) {
                            let _ = w.send(encoders.clone());
                        }
                    }
                    Ok(ev) => handler(ev),
                    Err(e) => log::warn!("unparseable recorder line {line:?}: {e}"),
                }
            }
            handler(RecorderEvent::Error {
                id: None,
                message: "recorder process exited".into(),
            });
        })?;
        log::info!("recorder started (pid {})", child.id());
        Ok(Proc { child, stdin })
    }

    pub fn send(&self, cmd: &RecorderCommand) -> Result<()> {
        let mut guard = self.proc.lock().unwrap();
        let proc = self.ensure_running(&mut guard)?;
        let mut line = serde_json::to_string(cmd)?;
        line.push('\n');
        if let Err(e) = proc.stdin.write_all(line.as_bytes()).and_then(|_| proc.stdin.flush()) {
            *guard = None;
            bail!("recorder pipe closed: {e}");
        }
        Ok(())
    }

    pub fn list_encoders(&self) -> Result<Vec<String>> {
        let (tx, rx) = mpsc::channel();
        self.encoder_waiters.lock().unwrap().push(tx);
        self.send(&RecorderCommand::ListEncoders)?;
        rx.recv_timeout(Duration::from_secs(15)).context("recorder did not answer")
    }

    /// Ask the recorder to finish and exit; kill it if it does not within a few seconds.
    pub fn shutdown(&self) {
        let mut guard = self.proc.lock().unwrap();
        if let Some(mut p) = guard.take() {
            let _ = writeln!(p.stdin, "{}", serde_json::to_string(&RecorderCommand::Shutdown).unwrap_or_default());
            let _ = p.stdin.flush();
            for _ in 0..50 {
                if p.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = p.child.kill();
        }
    }
}
