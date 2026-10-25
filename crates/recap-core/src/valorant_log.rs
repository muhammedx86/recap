//! VALORANT writes what the client does to `%LOCALAPPDATA%\VALORANT\Saved\Logs\ShooterGame.log`,
//! including the URLs of its own requests. From those lines, all on this computer:
//!
//! * the agent you select and lock in (`.../pregame/v1/matches/<id>/select|lock/<agent uuid>`),
//! * the match id (`.../core-game/v1/matches/<id>` or `.../pregame/v1/matches/<id>`),
//! * loop state changes (`Loopstate changed from MENUS to INGAME` ...),
//! * the end of the loading screen (`Transitioning from TransitionToInGame to InGame`).
//!
//! [`LogTail`] follows the file from a byte cursor (like `tail -f`), notices when the game starts
//! a new log, and turns new lines into [`LogEvent`]s.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// Read at most this much per poll (the log grows slowly; this bounds a catch-up read).
const MAX_READ: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// An agent was selected (`locked` = locked in).
    Agent {
        uuid: String,
        locked: bool,
    },
    MatchId(String),
    /// `Loopstate changed from <from> to <to>` (MENUS, PREGAME, INGAME).
    LoopState {
        from: String,
        to: String,
    },
    /// The loading screen is over: the match is on screen.
    LoadingDone,
}

/// `%LOCALAPPDATA%\VALORANT\Saved\Logs\ShooterGame.log`
pub fn default_log_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"VALORANT\Saved\Logs\ShooterGame.log"))
}

/// Parse one log line.
pub fn parse_line(line: &str) -> Vec<LogEvent> {
    let mut out = Vec::new();
    if let Some(rest) = line.split("Loopstate changed from ").nth(1) {
        let mut it = rest.split(" to ");
        if let (Some(from), Some(to)) = (it.next(), it.next()) {
            let word = |s: &str| s.trim().split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_string();
            out.push(LogEvent::LoopState {
                from: word(from),
                to: word(to),
            });
        }
    }
    if line.contains("Transitioning from TransitionToInGame to InGame") {
        out.push(LogEvent::LoadingDone);
    }
    for marker in ["core-game/v1/matches/", "pregame/v1/matches/"] {
        if let Some(rest) = line.split(marker).nth(1) {
            let id: String = rest.chars().take_while(|c| c.is_ascii_hexdigit() || *c == '-').collect();
            if is_uuid(&id) {
                if !out.iter().any(|e| matches!(e, LogEvent::MatchId(m) if *m == id)) {
                    out.push(LogEvent::MatchId(id.clone()));
                }
                let after = &rest[id.len()..];
                for (seg, locked) in [("/lock/", true), ("/select/", false)] {
                    if let Some(a) = after.strip_prefix(seg) {
                        let uuid: String = a.chars().take_while(|c| c.is_ascii_hexdigit() || *c == '-').collect();
                        if is_uuid(&uuid) {
                            out.push(LogEvent::Agent {
                                uuid: uuid.to_ascii_lowercase(),
                                locked,
                            });
                        }
                    }
                }
            }
        }
    }
    out
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4
}

/// Follows the log file across polls.
#[derive(Debug)]
pub struct LogTail {
    path: PathBuf,
    cursor: u64,
    header: Vec<u8>,
    partial: String,
}

impl LogTail {
    /// Start at the current end of the file: only lines written from now on are reported.
    pub fn new(path: &Path) -> Self {
        let mut t = Self {
            path: path.to_path_buf(),
            cursor: 0,
            header: Vec::new(),
            partial: String::new(),
        };
        if let Ok(meta) = std::fs::metadata(path) {
            t.cursor = meta.len();
            t.header = read_header(path);
        }
        t
    }

    /// New events since the last call.
    pub fn poll(&mut self) -> Vec<LogEvent> {
        let Ok(mut f) = File::open(&self.path) else { return vec![] };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        // The game started a new log (shorter file, or a different first line): read it from the top.
        let header = read_header(&self.path);
        if len < self.cursor || header != self.header {
            self.cursor = 0;
            self.partial.clear();
            self.header = header;
        }
        if len == self.cursor {
            return vec![];
        }
        let n = (len - self.cursor).min(MAX_READ);
        let mut buf = vec![0u8; n as usize];
        if f.seek(SeekFrom::Start(self.cursor)).is_err() || f.read_exact(&mut buf).is_err() {
            return vec![];
        }
        self.cursor += n;
        self.partial.push_str(&String::from_utf8_lossy(&buf));
        let mut out = Vec::new();
        // Keep an unfinished last line for the next poll.
        let complete = match self.partial.rfind('\n') {
            Some(i) => {
                let rest = self.partial.split_off(i + 1);
                std::mem::replace(&mut self.partial, rest)
            }
            None => return out,
        };
        for line in complete.lines() {
            out.extend(parse_line(line));
        }
        out
    }
}

fn read_header(path: &Path) -> Vec<u8> {
    let mut b = vec![0u8; 256];
    match File::open(path).and_then(|mut f| f.read(&mut b)) {
        Ok(n) => {
            // The first line ("Log file open, <time>") identifies the log.
            b.truncate(n);
            match b.iter().position(|&c| c == b'\n') {
                Some(i) => b[..i].to_vec(),
                None => b,
            }
        }
        Err(_) => vec![],
    }
}

/// `games/valorant/agents.json`: `{ "agents": { "<uuid>": "Name" } }`.
pub fn load_agent_names(game_dir: &Path) -> BTreeMap<String, String> {
    #[derive(serde::Deserialize)]
    struct File {
        agents: BTreeMap<String, String>,
    }
    std::fs::read_to_string(game_dir.join("agents.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<File>(&t).ok())
        .map(|f| f.agents.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const M: &str = "a1b2c3d4-0000-4000-8000-1234567890ab";
    const JETT: &str = "add6443a-41bd-e414-f6ad-e58d267f4e95";

    #[test]
    fn parses_the_lines_we_use() {
        let l = format!("[2026.10.24-21.03.11:123][ 42]LogPlatformCommon: Platform HTTP Query End. QueryName: [Pregame_SelectCharacter], URL [POST https://glz-na-1.na.a.pvp.net/pregame/v1/matches/{M}/select/{JETT}], Response Code: [200]");
        assert_eq!(
            parse_line(&l),
            vec![
                LogEvent::MatchId(M.into()),
                LogEvent::Agent {
                    uuid: JETT.into(),
                    locked: false
                }
            ]
        );
        let l = format!(
            "... URL [POST https://glz-na-1.na.a.pvp.net/pregame/v1/matches/{M}/lock/{}] ...",
            JETT.to_uppercase()
        );
        assert!(parse_line(&l).contains(&LogEvent::Agent {
            uuid: JETT.into(),
            locked: true
        }));
        assert_eq!(
            parse_line("[..]LogShooterGameState: Match Started, Loopstate changed from PREGAME to INGAME"),
            vec![LogEvent::LoopState {
                from: "PREGAME".into(),
                to: "INGAME".into()
            }]
        );
        assert_eq!(
            parse_line("[..]LogMapLoadModel: Transitioning from TransitionToInGame to InGame"),
            vec![LogEvent::LoadingDone]
        );
        assert_eq!(
            parse_line(&format!("GET .../core-game/v1/matches/{M}/loadouts")),
            vec![LogEvent::MatchId(M.into())]
        );
        assert!(parse_line("nothing interesting").is_empty());
    }

    #[test]
    fn tails_new_lines_and_follows_a_new_log() {
        let dir = std::env::temp_dir().join(format!("recap-vlog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("ShooterGame.log");
        std::fs::write(&p, "Log file open, 10/24/26 21:00:00\nLoopstate changed from MENUS to PREGAME\n").unwrap();
        let mut t = LogTail::new(&p);
        assert!(t.poll().is_empty(), "old lines are not replayed");
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "Loopstate changed from PREGAME to INGAME\nTransitioning from TransitionTo").unwrap();
        assert_eq!(
            t.poll(),
            vec![LogEvent::LoopState {
                from: "PREGAME".into(),
                to: "INGAME".into()
            }]
        );
        writeln!(f, "InGame to InGame").unwrap();
        assert_eq!(t.poll(), vec![LogEvent::LoadingDone], "a line split across polls is read whole");
        // The game restarts and writes a new, shorter log.
        std::fs::write(&p, "Log file open, 10/24/26 22:00:00\nLoopstate changed from MENUS to INGAME\n").unwrap();
        assert_eq!(
            t.poll(),
            vec![LogEvent::LoopState {
                from: "MENUS".into(),
                to: "INGAME".into()
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
