//! Post-match processors: external tools that read files the game wrote
//! (replays, demos, logs) after a match and report events.
//!
//! A processor is declared in a game's `game.json`:
//!
//! ```json
//! "postMatch": [{
//!   "type": "command",
//!   "id": "r6-replay",
//!   "command": "%LOCALAPPDATA%/Recap/tools/r6-to-recap.exe",
//!   "args": ["{watchDir}", "{matchStartUnixMs}", "{matchEndUnixMs}"],
//!   "watchDir": "%USERPROFILE%/Documents/My Games/Rainbow Six - Siege/replays"
//! }]
//! ```
//!
//! Placeholders substituted in `args`: `{recording}` (path of the .mp4),
//! `{matchStartUnixMs}`, `{matchEndUnixMs}`, `{gameDir}` (the game's data folder)
//! and `{watchDir}` (the expanded `watchDir`). Environment variables are
//! expanded in `command` and `watchDir` (`%VAR%` on Windows, `$VAR`/`${VAR}` elsewhere).
//!
//! The command must print a JSON array to stdout, one object per event:
//!
//! ```json
//! [
//!   { "kind": "kill",  "unixMs": 1761250000123, "meta": { "victim": "Ash" } },
//!   { "kind": "death", "tMs": 95000 }
//! ]
//! ```
//!
//! Each entry needs `kind` and either `tMs` (offset into the recording) or
//! `unixMs` (wall clock; converted using `matchStartUnixMs`). Entries outside
//! the match are dropped. Anything printed to stderr is logged. A non-zero exit
//! code or a timeout is an error.

use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::{
    events::{EventSource, GameEvent},
    games::PostMatchDef,
};

#[derive(Debug, Clone)]
pub struct PostMatchContext {
    pub recording: PathBuf,
    pub match_start_unix_ms: u64,
    pub match_end_unix_ms: u64,
    pub game_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutputEvent {
    kind: String,
    #[serde(default)]
    t_ms: Option<u64>,
    #[serde(default)]
    unix_ms: Option<u64>,
    #[serde(default)]
    meta: serde_json::Value,
}

/// Expand environment variables: `%VAR%` on Windows, `$VAR` / `${VAR}` elsewhere.
/// Unknown variables are left as written.
pub fn expand_env(s: &str) -> String {
    if cfg!(windows) {
        expand_percent(s)
    } else {
        expand_dollar(s)
    }
}

fn expand_percent(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j)
                if j > 0
                    && after[..j]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '(' || c == ')') =>
            {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn expand_dollar(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let (name, end) = if chars[i + 1] == '{' {
                match chars[i + 2..].iter().position(|&c| c == '}') {
                    Some(p) => (chars[i + 2..i + 2 + p].iter().collect::<String>(), i + 3 + p),
                    None => (String::new(), i + 1),
                }
            } else {
                let len = chars[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .count();
                (chars[i + 1..i + 1 + len].iter().collect(), i + 1 + len)
            };
            if !name.is_empty() {
                if let Ok(v) = std::env::var(&name) {
                    out.push_str(&v);
                    i = end;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn substitute(arg: &str, ctx: &PostMatchContext, watch_dir: &str) -> String {
    arg.replace("{recording}", &ctx.recording.to_string_lossy())
        .replace("{matchStartUnixMs}", &ctx.match_start_unix_ms.to_string())
        .replace("{matchEndUnixMs}", &ctx.match_end_unix_ms.to_string())
        .replace("{gameDir}", &ctx.game_dir.to_string_lossy())
        .replace("{watchDir}", watch_dir)
}

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

/// Parse a processor's stdout into events relative to the recording.
pub fn parse_output(id: &str, stdout: &str, ctx: &PostMatchContext) -> Result<Vec<GameEvent>> {
    let entries: Vec<OutputEvent> = serde_json::from_str(stdout.trim()).context("processor output must be a JSON array of events")?;
    let duration = ctx.match_end_unix_ms.saturating_sub(ctx.match_start_unix_ms);
    let mut out = Vec::new();
    for e in entries {
        if e.kind.trim().is_empty() {
            continue;
        }
        let t = match (e.t_ms, e.unix_ms) {
            (Some(t), _) => Some(t),
            (None, Some(u)) if u >= ctx.match_start_unix_ms => Some(u - ctx.match_start_unix_ms),
            _ => None,
        };
        let Some(t) = t.filter(|t| duration == 0 || *t <= duration) else {
            continue;
        };
        out.push(GameEvent::new(e.kind, t, EventSource::PostMatch { processor: id.to_string() }).with_meta(e.meta));
    }
    out.sort_by_key(|e| e.t_ms);
    Ok(out)
}

/// Run a post-match processor and return its events.
pub fn run_processor(def: &PostMatchDef, ctx: &PostMatchContext, timeout: Duration) -> Result<Vec<GameEvent>> {
    let PostMatchDef::Command {
        id,
        command,
        args,
        watch_dir,
    } = def;
    let watch = watch_dir.as_deref().map(expand_env).unwrap_or_default();
    let program = expand_env(command);
    let mut cmd = Command::new(&program);
    no_window(&mut cmd);
    cmd.args(args.iter().map(|a| substitute(a, ctx, &watch)))
        .current_dir(&ctx.game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting post-match processor {id} ({program})"))?;

    let mut stdout = child.stdout.take().context("no stdout")?;
    let mut stderr = child.stderr.take().context("no stderr")?;
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("post-match processor {id} timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !stderr.trim().is_empty() {
        log::info!("post-match {id}: {}", stderr.trim());
    }
    if !status.success() {
        bail!("post-match processor {id} exited with {status}: {}", stderr.trim());
    }
    parse_output(id, &stdout, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> PostMatchContext {
        PostMatchContext {
            recording: dir.join("rec.mp4"),
            match_start_unix_ms: 1_000_000,
            match_end_unix_ms: 1_600_000,
            game_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn parses_and_filters_output() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let ev = parse_output(
            "p",
            r#"[{"kind":"kill","unixMs":1100000,"meta":{"victim":"Ash"}},{"kind":"death","tMs":5000},{"kind":"kill","unixMs":10},{"kind":"kill","tMs":9999999},{"kind":"","tMs":1},{"kind":"x"}]"#,
            &c,
        )
        .unwrap();
        assert_eq!(
            ev.iter().map(|e| (e.kind.as_str(), e.t_ms)).collect::<Vec<_>>(),
            vec![("death", 5000), ("kill", 100_000)]
        );
        assert_eq!(ev[1].meta["victim"], "Ash");
        assert_eq!(ev[0].source, EventSource::PostMatch { processor: "p".into() });
        assert!(parse_output("p", "not json", &c).is_err());
    }

    #[test]
    fn expands_env() {
        std::env::set_var("RECAP_TEST_DIR", "/x/y");
        assert_eq!(
            expand_dollar("$RECAP_TEST_DIR/a ${RECAP_TEST_DIR}b $NOPE_RECAP $"),
            "/x/y/a /x/yb $NOPE_RECAP $"
        );
        assert_eq!(expand_percent("%RECAP_TEST_DIR%/a %NOPE_RECAP% 50%"), "/x/y/a %NOPE_RECAP% 50%");
    }

    #[cfg(unix)]
    #[test]
    fn runs_command_with_substitutions_and_timeout() {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("proc.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"args: $@\" >&2\necho \"[{\\\"kind\\\":\\\"kill\\\",\\\"unixMs\\\":$(( $2 + 1500 )),\\\"meta\\\":{\\\"dir\\\":\\\"$3\\\"}}]\"\n",
        )
        .unwrap();
        std::env::set_var("RECAP_WATCH", "/replays");
        let def = PostMatchDef::Command {
            id: "test".into(),
            command: "sh".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                "{recording}".into(),
                "{matchStartUnixMs}".into(),
                "{watchDir}".into(),
            ],
            watch_dir: Some("$RECAP_WATCH/r6".into()),
        };
        let ev = run_processor(&def, &ctx(d.path()), Duration::from_secs(10)).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].t_ms, 1500);
        assert_eq!(ev[0].meta["dir"], "/replays/r6");

        let slow = PostMatchDef::Command {
            id: "slow".into(),
            command: "sleep".into(),
            args: vec!["5".into()],
            watch_dir: None,
        };
        let t = Instant::now();
        assert!(run_processor(&slow, &ctx(d.path()), Duration::from_millis(300))
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(3));

        let failing = PostMatchDef::Command {
            id: "f".into(),
            command: "false".into(),
            args: vec![],
            watch_dir: None,
        };
        assert!(run_processor(&failing, &ctx(d.path()), Duration::from_secs(5)).is_err());
    }
}
