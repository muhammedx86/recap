//! Rocket League: the game's official local Stats API (Psyonix, "MatchStatsExporter").
//!
//! Enabled by `PacketSendRate` > 0 in `<install>\TAGame\Config\DefaultStatsAPI.ini`. The game then
//! serves a stream of JSON messages on `127.0.0.1:<Port>` (49123 by default) while a match runs:
//! `{"Event": "...", "Data": {...}}`, where `Data` may itself be a JSON-encoded string. Psyonix
//! calls it a web socket; clients in the wild read it as raw concatenated JSON over TCP, so
//! [`JsonStream`] frames objects out of whatever bytes arrive.
//!
//! [`RlTracker`] turns those messages into Recap events for the local player (goals, assists,
//! saves, epic saves, shots, demos, goals against, MVP), match start/end, and details (arena,
//! mode, score, result, your goals/assists/saves/shots).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use crate::{
    details::{field, result_from_score, SnapshotEmitter},
    events::{kind, EventSource, GameEvent},
};

pub const SOURCE_NAME: &str = "rocket-league-stats";
pub const DEFAULT_PORT: u16 = 49123;
/// Updates per second Recap asks for. Events are sent the moment they happen regardless; this
/// only paces the periodic state (score, stats), so a low rate is plenty and costs nothing.
pub const PACKET_RATE: f32 = 10.0;
pub const INI_SECTION: &str = "[TAGame.MatchStatsExporter_TA]";

/// Rocket League event kinds (labels and icons come from `games/rocket-league/game.json`).
pub mod rl_kind {
    pub const GOAL: &str = "goal";
    pub const SAVE: &str = "save";
    pub const EPIC_SAVE: &str = "epic_save";
    pub const SHOT: &str = "shot";
    pub const DEMO: &str = "demo";
    pub const DEMOED: &str = "demoed";
    pub const GOAL_AGAINST: &str = "goal_against";
}

// ---------------------------------------------------------------------------------------------
// Framing

/// Splits a byte stream of concatenated JSON objects into whole objects.
#[derive(Debug, Default)]
pub struct JsonStream {
    buf: Vec<u8>,
}

impl JsonStream {
    /// Limit for one unfinished object (a broken stream must not grow forever).
    const MAX: usize = 4 * 1024 * 1024;

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut consumed = 0;
        loop {
            let rest = &self.buf[consumed..];
            // Skip anything before the next object (newlines, NULs, frame headers).
            let Some(start) = rest.iter().position(|&b| b == b'{') else {
                consumed = self.buf.len();
                break;
            };
            match object_end(&rest[start..]) {
                Some(len) => {
                    let obj = &rest[start..start + len];
                    if let Ok(v) = serde_json::from_slice::<Value>(obj) {
                        out.push(v);
                    }
                    consumed += start + len;
                }
                None => {
                    consumed += start;
                    break;
                }
            }
        }
        self.buf.drain(..consumed);
        if self.buf.len() > Self::MAX {
            self.buf.clear();
        }
        out
    }
}

/// Length of the JSON object at the start of `b` (which begins with `{`), if it is complete.
fn object_end(b: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `(event name, data)` from one message; `Data` is decoded again when it arrives as a string.
pub fn envelope(msg: &Value) -> Option<(String, Value)> {
    let ev = msg.get("Event")?.as_str()?.to_string();
    let data = match msg.get("Data") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    Some((ev, data))
}

// ---------------------------------------------------------------------------------------------
// DefaultStatsAPI.ini

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IniState {
    pub rate: Option<f32>,
    pub port: Option<u16>,
}

impl IniState {
    pub fn enabled(&self) -> bool {
        self.rate.is_some_and(|r| r > 0.0)
    }
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }
}

pub fn stats_ini_path(install: &Path) -> PathBuf {
    install.join("TAGame").join("Config").join("DefaultStatsAPI.ini")
}

fn ini_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (k, v) = line.split_once('=')?;
    k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
}

pub fn parse_ini(text: &str) -> IniState {
    let mut s = IniState::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(v) = ini_value(line, "PacketSendRate") {
            s.rate = v.parse().ok();
        }
        if let Some(v) = ini_value(line, "Port") {
            s.port = v.parse().ok();
        }
    }
    s
}

/// The ini text with `PacketSendRate` set to `rate` (everything else kept, line endings too).
pub fn with_rate(text: &str, rate: f32) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let value = format!("PacketSendRate={rate}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(l) = lines
        .iter_mut()
        .find(|l| !l.trim_start().starts_with(';') && ini_value(l.trim(), "PacketSendRate").is_some())
    {
        *l = value;
    } else if let Some(i) = lines.iter().position(|l| l.trim().eq_ignore_ascii_case(INI_SECTION)) {
        lines.insert(i + 1, value);
    } else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(INI_SECTION.to_string());
        lines.push(format!("Port={DEFAULT_PORT}"));
        lines.push(value);
    }
    let mut out = lines.join(nl);
    out.push_str(nl);
    out
}

/// `<install>` from `<install>\Binaries\Win64\RocketLeague.exe`.
pub fn install_from_exe(exe: &Path) -> Option<PathBuf> {
    let win64 = exe.parent()?;
    let binaries = win64.parent()?;
    binaries
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("Binaries")
        .then(|| binaries.parent().map(Path::to_path_buf))
        .flatten()
}

/// Rocket League installs listed by the Epic Games launcher (`*.item` manifests, app "Sugar").
pub fn epic_installs(manifest_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(manifest_dir) else { return vec![] };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| {
            v["AppName"].as_str() == Some("Sugar")
                || v["DisplayName"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().contains("rocket league"))
        })
        .filter_map(|v| v["InstallLocation"].as_str().map(PathBuf::from))
        .collect()
}

/// Rocket League installs in Steam libraries.
pub fn steam_installs(steam_root: &Path) -> Vec<PathBuf> {
    crate::cs2::steam_libraries(steam_root)
        .into_iter()
        .map(|lib| lib.join("steamapps").join("common").join("rocketleague"))
        .filter(|p| p.is_dir())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Names

/// Arena display name from the API's map name ("Stadium_P" -> "DFH Stadium").
pub fn arena_name(raw: &str) -> String {
    const ARENAS: &[(&str, &str)] = &[
        ("chn_stadium", "Forbidden Temple"),
        ("eurostadium", "Mannfield"),
        ("utopiastadium", "Utopia Coliseum"),
        ("throwbackstadium", "Throwback Stadium"),
        ("hoopsstadium", "Dunk House"),
        ("hoopsstreet", "The Block"),
        ("shattershot", "Core 707"),
        ("stadium", "DFH Stadium"),
        ("cs_", "Champions Field"),
        ("park_", "Beckwith Park"),
        ("haunted_trainstation", "Urban Central"),
        ("trainstation", "Urban Central"),
        ("wasteland", "Wasteland"),
        ("neotokyo", "Neo Tokyo"),
        ("arc_", "Starbase ARC"),
        ("beach", "Salty Shores"),
        ("farm", "Farmstead"),
        ("underwater", "AquaDome"),
        ("music", "Neon Fields"),
        ("street", "Sovereign Heights"),
        ("outlaw", "Deadeye Canyon"),
        ("woods", "Drift Woods"),
        ("labs_", "Rocket Labs"),
    ];
    let l = raw.trim().to_lowercase();
    if let Some((_, n)) = ARENAS.iter().find(|(p, _)| l.starts_with(p)) {
        return n.to_string();
    }
    // Unknown: "Some_Arena_P" -> "Some Arena"; already-pretty names stay as they are.
    let s = raw.trim();
    let s = s.strip_suffix("_P").or_else(|| s.strip_suffix("_p")).unwrap_or(s);
    s.replace('_', " ")
}

// ---------------------------------------------------------------------------------------------
// Tracker

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PlayerKey {
    name: String,
    team: i64,
    shortcut: i64,
}

impl PlayerKey {
    fn of(v: &Value) -> Option<Self> {
        Some(Self {
            name: v["Name"].as_str()?.to_string(),
            team: v["TeamNum"].as_i64().unwrap_or(-1),
            shortcut: v["Shortcut"].as_i64().unwrap_or(-1),
        })
    }
    /// Same player: shortcut and team when both are known, else the name.
    fn is(&self, other: &PlayerKey) -> bool {
        if self.shortcut >= 0 && other.shortcut >= 0 {
            self.shortcut == other.shortcut && self.team == other.team
        } else {
            self.name == other.name
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Stats {
    goals: i64,
    assists: i64,
    saves: i64,
    shots: i64,
    demos: i64,
    score: i64,
}

impl Stats {
    fn of(p: &Value) -> Self {
        let n = |k: &str| p[k].as_i64().unwrap_or(0);
        Self {
            goals: n("Goals"),
            assists: n("Assists"),
            saves: n("Saves"),
            shots: n("Shots"),
            demos: n("Demos"),
            score: n("Score"),
        }
    }
}

/// A play waiting one tick so a later statfeed message on the same tick can refine it
/// (an epic save, an aerial goal).
#[derive(Debug)]
struct Pending {
    ev: GameEvent,
    at_ms: u64,
}

/// What the tracker knows about the match in progress.
#[derive(Debug, Default)]
struct MatchState {
    on: bool,
    started: bool,
    ended: bool,
    my_team: Option<i64>,
    arena: Option<String>,
    team_sizes: (usize, usize),
    score: Option<(i64, i64)>,
    winner: Option<i64>,
    my_stats: Option<Stats>,
    my_name: Option<String>,
    /// Plays accounted for so far (statfeed + fallbacks), per counter.
    seen: Stats,
    /// When a counter first ran ahead of `seen` (monotonic ms), per kind.
    deficit_since: HashMap<&'static str, (u64, Option<u64>)>,
    target_votes: HashMap<PlayerKey, u32>,
    me: Option<PlayerKey>,
    was_demolished: bool,
    last_demoed_ms: Option<u64>,
}

/// Follows the Stats API across matches. Identity of the local player carries over between
/// matches through its platform id.
#[derive(Debug, Default)]
pub struct RlTracker {
    m: MatchState,
    my_primary_id: Option<String>,
    pending: Vec<Pending>,
    details: SnapshotEmitter,
    last_msg_ms: Option<u64>,
}

/// Fallback events wait this long for the statfeed to report a play before counting it from the
/// scoreboard instead.
const FALLBACK_MS: u64 = 1_500;
/// A play is held this long for refinements from the same tick.
const HOLD_MS: u64 = 120;
/// A match with no message for this long is over (the game closed the socket or crashed).
pub const SILENCE_MS: u64 = 10_000;

fn ev(k: &str, rec_ms: u64, meta: Value) -> GameEvent {
    GameEvent::new(k, rec_ms, EventSource::Api { name: SOURCE_NAME.into() }).with_meta(meta)
}

impl RlTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a real match (two teams on the field, no winner yet) is on.
    pub fn in_match(&self, now_ms: u64) -> bool {
        self.m.on && !self.m.ended && self.last_msg_ms.is_some_and(|t| now_ms.saturating_sub(t) < SILENCE_MS)
    }

    /// A new recording begins: its details start from scratch.
    pub fn new_recording(&mut self) {
        self.details = SnapshotEmitter::default();
    }

    fn reset_match(&mut self) {
        let keep_id = self.my_primary_id.clone();
        self.m = MatchState::default();
        self.pending.clear();
        self.my_primary_id = keep_id;
    }

    fn is_me(&self, v: &Value) -> bool {
        match (&self.m.me, PlayerKey::of(v)) {
            (Some(me), Some(k)) => me.is(&k),
            _ => false,
        }
    }

    /// Feed one message. `now_ms` is a monotonic clock; `rec_ms` is the position in the running
    /// recording (None when nothing records: state is still followed, no events are made).
    pub fn ingest(&mut self, msg: &Value, now_ms: u64, rec_ms: Option<u64>) -> Vec<GameEvent> {
        let Some((name, data)) = envelope(msg) else { return vec![] };
        self.last_msg_ms = Some(now_ms);
        let mut out = Vec::new();
        match name.as_str() {
            "MatchCreated" | "MatchInitialized" => {
                if self.m.ended || !self.m.on {
                    self.reset_match();
                }
            }
            "CountdownBegin" | "RoundStarted" => {
                if self.m.ended {
                    self.reset_match();
                }
                if !self.m.started && self.m.on {
                    self.m.started = true;
                    if let Some(t) = rec_ms {
                        out.push(ev(kind::MATCH_START, t, json!({ "arena": self.m.arena })));
                    }
                }
            }
            "UpdateState" => {
                self.update_state(&data, now_ms, rec_ms, &mut out);
            }
            "GoalScored" => self.goal_scored(&data, now_ms, rec_ms),
            "StatfeedEvent" => self.statfeed(&data, now_ms, rec_ms),
            "MatchEnded" => {
                let winner = data["WinnerTeamNum"].as_i64();
                self.m.winner = winner.or(self.m.winner);
                self.flush(now_ms, true, &mut out);
                if self.m.on && !self.m.ended {
                    if let Some(t) = rec_ms {
                        let result = self.result();
                        out.push(ev(kind::MATCH_END, t, json!({ "winner": winner, "result": result })));
                    }
                }
                self.m.ended = true;
            }
            "MatchDestroyed" => {
                self.flush(now_ms, true, &mut out);
                // Left mid-match or back to the menu: the match is over either way.
                if self.m.on {
                    self.m.ended = true;
                }
            }
            _ => {}
        }
        if let Some(t) = rec_ms {
            out.extend(self.details.update(self.snapshot(), t, SOURCE_NAME));
        }
        out
    }

    fn result(&self) -> Option<&'static str> {
        let (w, team) = (self.m.winner?, self.m.my_team?);
        Some(if w == team { "win" } else { "loss" })
    }

    fn update_state(&mut self, data: &Value, now_ms: u64, rec_ms: Option<u64>, out: &mut Vec<GameEvent>) {
        let game = &data["Game"];
        let players = data["Players"].as_array().cloned().unwrap_or_default();
        let blue = players.iter().filter(|p| p["TeamNum"].as_i64() == Some(0)).count();
        let orange = players.iter().filter(|p| p["TeamNum"].as_i64() == Some(1)).count();
        // Free play and training have one side only; a real match has both.
        let both_sides = blue > 0 && orange > 0;
        if self.m.ended && both_sides && !game["bHasWinner"].as_bool().unwrap_or(false) {
            // The next match (e.g. a rematch) began without a MatchCreated we saw.
            self.reset_match();
        }
        if both_sides && !self.m.ended {
            self.m.on = true;
        }
        self.m.team_sizes = (blue, orange);
        if let Some(a) = game["Arena"].as_str().filter(|a| !a.is_empty()) {
            self.m.arena = Some(arena_name(a));
        }
        if game["bHasWinner"].as_bool() == Some(true) {
            self.m.winner = game["Winner"].as_i64().or(self.m.winner);
        }

        // Who am I? The platform id from an earlier match, else whom the camera follows.
        if self.m.me.is_none() {
            if let Some(id) = self.my_primary_id.as_deref() {
                if let Some(p) = players.iter().find(|p| p["PrimaryId"].as_str() == Some(id)) {
                    self.m.me = PlayerKey::of(p);
                }
            }
        }
        let replay = game["bReplay"].as_bool().unwrap_or(false);
        if !replay && game["bHasTarget"].as_bool() == Some(true) {
            if let Some(k) = PlayerKey::of(&game["Target"]) {
                *self.m.target_votes.entry(k).or_default() += 1;
            }
        }
        // In a 1v1 (or alone on a team) the side whose boost we can see is ours.
        let with_boost: Vec<&Value> = players.iter().filter(|p| p.get("Boost").is_some()).collect();
        if with_boost.len() == 1 && both_sides {
            if let Some(k) = PlayerKey::of(with_boost[0]) {
                *self.m.target_votes.entry(k).or_default() += 2;
            }
        }
        if !self.m.target_votes.is_empty() {
            let best = self.m.target_votes.iter().max_by_key(|(_, v)| **v).map(|(k, _)| k.clone());
            let id_locked = self.my_primary_id.is_some() && self.m.me.is_some();
            if !id_locked {
                self.m.me = best;
            }
        }

        let me = players.iter().find(|p| self.is_me(p)).cloned();
        if let Some(p) = &me {
            self.m.my_team = p["TeamNum"].as_i64();
            self.m.my_name = p["Name"].as_str().map(str::to_string);
            if let Some(id) = p["PrimaryId"].as_str() {
                // Bots and offline players report "Unknown|0|0".
                if !id.starts_with("Unknown") && self.m.target_votes.values().sum::<u32>() >= 5 {
                    self.my_primary_id = Some(id.to_string());
                }
            }
            let st = Stats::of(p);
            if self.m.my_stats.is_none() {
                // Joined mid-match (or recording began late): what happened before is not ours.
                self.m.seen = st;
            }
            self.m.my_stats = Some(st);
            // Demolished by someone: `Attacker` appears on our player.
            let demolished = p.get("Attacker").is_some_and(|a| !a.is_null());
            if demolished && !self.m.was_demolished {
                let recent = self.m.last_demoed_ms.is_some_and(|t| now_ms.saturating_sub(t) < 3_000);
                if !recent {
                    self.m.last_demoed_ms = Some(now_ms);
                    if let Some(t) = rec_ms {
                        let by = p["Attacker"]["Name"].as_str();
                        self.hold(ev(rl_kind::DEMOED, t, json!({ "by": by })), now_ms);
                    }
                }
            }
            self.m.was_demolished = demolished;
        }

        // Score, mine first.
        if let (Some(team), Some(teams)) = (self.m.my_team, game["Teams"].as_array()) {
            let score_of = |n: i64| {
                teams
                    .iter()
                    .find(|t| t["TeamNum"].as_i64() == Some(n))
                    .and_then(|t| t["Score"].as_i64())
            };
            if let (Some(a), Some(b)) = (score_of(team), score_of(1 - team)) {
                self.m.score = Some((a, b));
            }
        }

        self.flush(now_ms, false, out);
        self.fallbacks(now_ms, rec_ms, out);
    }

    /// Plays the statfeed missed, counted from the scoreboard after a grace period.
    fn fallbacks(&mut self, now_ms: u64, rec_ms: Option<u64>, out: &mut Vec<GameEvent>) {
        let Some(st) = self.m.my_stats else { return };
        let pairs: [(&'static str, i64, i64); 5] = [
            (rl_kind::GOAL, st.goals, self.m.seen.goals),
            (kind::ASSIST, st.assists, self.m.seen.assists),
            (rl_kind::SAVE, st.saves, self.m.seen.saves),
            (rl_kind::SHOT, st.shots, self.m.seen.shots),
            (rl_kind::DEMO, st.demos, self.m.seen.demos),
        ];
        for (k, have, seen) in pairs {
            if have < seen {
                // Counters went down (a new match): start over from them.
                self.set_seen(k, have);
                self.m.deficit_since.remove(k);
                continue;
            }
            if have == seen {
                self.m.deficit_since.remove(k);
                continue;
            }
            let (since, at) = *self.m.deficit_since.entry(k).or_insert((now_ms, rec_ms));
            if now_ms.saturating_sub(since) >= FALLBACK_MS {
                if let Some(t) = at.or(rec_ms) {
                    for _ in 0..(have - seen).min(5) {
                        out.push(ev(k, t, json!({ "from": "scoreboard" })));
                    }
                }
                self.set_seen(k, have);
                self.m.deficit_since.remove(k);
            }
        }
    }

    fn set_seen(&mut self, k: &str, v: i64) {
        let s = &mut self.m.seen;
        match k {
            rl_kind::GOAL => s.goals = v,
            kind::ASSIST => s.assists = v,
            rl_kind::SAVE => s.saves = v,
            rl_kind::SHOT => s.shots = v,
            rl_kind::DEMO => s.demos = v,
            _ => {}
        }
    }

    fn count(&mut self, k: &str) {
        let s = &mut self.m.seen;
        match k {
            rl_kind::GOAL => s.goals += 1,
            kind::ASSIST => s.assists += 1,
            rl_kind::SAVE | rl_kind::EPIC_SAVE => s.saves += 1,
            rl_kind::SHOT => s.shots += 1,
            rl_kind::DEMO => s.demos += 1,
            _ => {}
        }
        self.m.deficit_since.remove(match k {
            rl_kind::EPIC_SAVE => rl_kind::SAVE,
            other => other,
        });
    }

    fn hold(&mut self, e: GameEvent, now_ms: u64) {
        let k = e.kind.clone();
        self.count(&k);
        self.pending.push(Pending { ev: e, at_ms: now_ms });
    }

    fn flush(&mut self, now_ms: u64, all: bool, out: &mut Vec<GameEvent>) {
        let (ready, keep): (Vec<_>, Vec<_>) = self
            .pending
            .drain(..)
            .partition(|p| all || now_ms.saturating_sub(p.at_ms) >= HOLD_MS);
        self.pending = keep;
        out.extend(ready.into_iter().map(|p| p.ev));
    }

    fn goal_scored(&mut self, d: &Value, now_ms: u64, rec_ms: Option<u64>) {
        let scorer = &d["Scorer"];
        let assister = d.get("Assister").filter(|a| !a.is_null());
        let speed = d["GoalSpeed"].as_f64().map(|s| (s * 10.0).round() / 10.0);
        let scorer_team = scorer["TeamNum"].as_i64();
        let Some(t) = rec_ms else {
            // Not recording: keep the counters in step so nothing is double counted later.
            if self.is_me(scorer) {
                self.count(rl_kind::GOAL);
            }
            if assister.is_some_and(|a| self.is_me(a)) {
                self.count(kind::ASSIST);
            }
            return;
        };
        if self.is_me(scorer) {
            let meta = json!({ "speed": speed, "assist": assister.and_then(|a| a["Name"].as_str()) });
            self.hold(ev(rl_kind::GOAL, t, meta), now_ms);
        } else if assister.is_some_and(|a| self.is_me(a)) {
            let meta = json!({ "scorer": scorer["Name"].as_str(), "speed": speed });
            self.hold(ev(kind::ASSIST, t, meta), now_ms);
        } else if scorer_team.is_some() && self.m.my_team.is_some() && scorer_team != self.m.my_team {
            let meta = json!({ "scorer": scorer["Name"].as_str(), "speed": speed });
            self.hold(ev(rl_kind::GOAL_AGAINST, t, meta), now_ms);
        }
    }

    fn statfeed(&mut self, d: &Value, now_ms: u64, rec_ms: Option<u64>) {
        let name: String = d["EventName"]
            .as_str()
            .or_else(|| d["Type"].as_str())
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        let main_me = self.is_me(&d["MainTarget"]);
        let second_me = d.get("SecondaryTarget").is_some_and(|s| self.is_me(s));
        let style = |n: &str| -> Option<&'static str> {
            Some(match n {
                "aerialgoal" => "Aerial goal",
                "bicyclegoal" => "Bicycle goal",
                "longgoal" => "Long goal",
                "turtlegoal" => "Turtle goal",
                "backwallgoal" => "Backwall goal",
                "poolshot" => "Pool shot",
                "overtimegoal" => "Overtime goal",
                "hattrick" => "Hat trick",
                "swishgoal" => "Swish goal",
                _ => return None,
            })
        };
        // Refinements of a play already held from this tick.
        if main_me {
            if let Some(s) = style(&name) {
                if let Some(p) = self.pending.iter_mut().rev().find(|p| p.ev.kind == rl_kind::GOAL) {
                    p.ev.meta["style"] = json!(s);
                }
                return;
            }
        }
        let Some(t) = rec_ms else {
            match name.as_str() {
                "save" | "epicsave" if main_me => self.count(rl_kind::SAVE),
                "shot" | "shotongoal" if main_me => self.count(rl_kind::SHOT),
                "demolish" | "demolition" | "demo" if main_me => self.count(rl_kind::DEMO),
                _ => {}
            }
            return;
        };
        match name.as_str() {
            "save" if main_me => self.hold(ev(rl_kind::SAVE, t, json!({})), now_ms),
            "epicsave" if main_me => {
                // The same save may have been reported plainly on this tick: upgrade it.
                if let Some(p) = self
                    .pending
                    .iter_mut()
                    .rev()
                    .find(|p| p.ev.kind == rl_kind::SAVE && now_ms.saturating_sub(p.at_ms) < 1_000)
                {
                    p.ev.kind = rl_kind::EPIC_SAVE.to_string();
                } else {
                    self.hold(ev(rl_kind::EPIC_SAVE, t, json!({})), now_ms);
                }
            }
            "shot" | "shotongoal" if main_me => self.hold(ev(rl_kind::SHOT, t, json!({})), now_ms),
            "demolish" | "demolition" | "demo" => {
                if main_me {
                    let victim = d["SecondaryTarget"]["Name"].as_str();
                    self.hold(ev(rl_kind::DEMO, t, json!({ "victim": victim })), now_ms);
                } else if second_me {
                    let recent = self.m.last_demoed_ms.is_some_and(|x| now_ms.saturating_sub(x) < 3_000);
                    if !recent {
                        self.m.last_demoed_ms = Some(now_ms);
                        let by = d["MainTarget"]["Name"].as_str();
                        self.hold(ev(rl_kind::DEMOED, t, json!({ "by": by })), now_ms);
                    }
                }
            }
            "mvp" if main_me => self.hold(ev(kind::MVP, t, json!({})), now_ms),
            _ => {}
        }
    }

    /// Arena, mode, score, result and my stats.
    fn snapshot(&self) -> std::collections::BTreeMap<String, String> {
        let mut d = std::collections::BTreeMap::new();
        if !self.m.on {
            return d;
        }
        if let Some(a) = &self.m.arena {
            d.insert(field::MAP.to_string(), a.clone());
        }
        let (b, o) = self.m.team_sizes;
        if b > 0 && o > 0 {
            let n = b.max(o);
            d.insert(field::MODE.to_string(), format!("{n}v{n}"));
        }
        if let Some(n) = &self.m.my_name {
            d.insert(field::PLAYER.to_string(), n.clone());
        }
        if let Some((a, b)) = self.m.score {
            d.insert(field::SCORE.to_string(), format!("{a}-{b}"));
            if self.m.ended {
                let r = self.result().unwrap_or_else(|| result_from_score(a, b));
                d.insert(field::RESULT.to_string(), r.to_string());
            }
        }
        if let Some(s) = self.m.my_stats {
            d.insert("goals".into(), s.goals.to_string());
            d.insert(field::ASSISTS.to_string(), s.assists.to_string());
            d.insert("saves".into(), s.saves.to_string());
            d.insert("shots".into(), s.shots.to_string());
            d.insert("points".into(), s.score.to_string());
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(name: &str, team: i64, sc: i64, goals: i64, saves: i64, shots: i64, boost: bool) -> Value {
        let mut p = json!({
            "Name": name, "PrimaryId": format!("Epic|{name}|0"), "Shortcut": sc, "TeamNum": team,
            "Score": goals * 100 + saves * 50, "Goals": goals, "Shots": shots, "Assists": 0, "Saves": saves,
            "Touches": 3, "CarTouches": 0, "Demos": 0
        });
        if boost {
            p["Boost"] = json!(50);
        }
        p
    }

    fn update(players: Vec<Value>, target: &str, sc: i64, team: i64, score: (i64, i64)) -> Value {
        json!({ "Event": "UpdateState", "Data": {
            "MatchGuid": "G1",
            "Players": players,
            "Game": {
                "Teams": [ { "Name": "Blue", "TeamNum": 0, "Score": score.0 }, { "Name": "Orange", "TeamNum": 1, "Score": score.1 } ],
                "TimeSeconds": 250, "bOvertime": false, "Arena": "Stadium_P", "bReplay": false,
                "bHasWinner": false, "Winner": "", "bHasTarget": true,
                "Target": { "Name": target, "Shortcut": sc, "TeamNum": team }
            }
        }})
    }

    #[test]
    fn frames_concatenated_and_split_json() {
        let mut s = JsonStream::default();
        let a = r#"{"Event":"A","Data":"{\"x\":\"}{\"}"}{"Event":"B","Data":{}}"#;
        let (p1, p2) = a.split_at(17);
        assert!(s.push(p1.as_bytes()).is_empty());
        let got = s.push(p2.as_bytes());
        assert_eq!(got.len(), 2);
        let (name, data) = envelope(&got[0]).unwrap();
        assert_eq!(name, "A");
        assert_eq!(data["x"], "}{", "a string Data is decoded again");
        assert_eq!(envelope(&got[1]).unwrap().0, "B");
        // Junk between objects (newlines, frame bytes) is skipped.
        let got = s.push(b"\n\x81\x05{\"Event\":\"C\"}\r\n");
        assert_eq!(envelope(&got[0]).unwrap().0, "C");
    }

    #[test]
    fn ini_is_switched_on_without_touching_the_rest() {
        let text = "[TAGame.MatchStatsExporter_TA]\r\nPort=49123\r\nPacketSendRate=0\r\n";
        assert!(!parse_ini(text).enabled());
        let on = with_rate(text, PACKET_RATE);
        assert_eq!(on, "[TAGame.MatchStatsExporter_TA]\r\nPort=49123\r\nPacketSendRate=10\r\n");
        let st = parse_ini(&on);
        assert!(st.enabled());
        assert_eq!(st.port(), 49123);
        // Missing key or section.
        let on = with_rate("[TAGame.MatchStatsExporter_TA]\nPort=5000\n", 10.0);
        assert_eq!(
            parse_ini(&on),
            IniState {
                rate: Some(10.0),
                port: Some(5000)
            }
        );
        let on = with_rate("", 10.0);
        assert!(parse_ini(&on).enabled() && on.contains(INI_SECTION));
    }

    #[test]
    fn install_paths() {
        let exe = Path::new("/games/rocketleague/Binaries/Win64/RocketLeague.exe");
        assert_eq!(install_from_exe(exe), Some(PathBuf::from("/games/rocketleague")));
        assert_eq!(install_from_exe(Path::new("/x/RocketLeague.exe")), None);
        let dir = std::env::temp_dir().join(format!("recap-rl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.item"),
            r#"{"AppName":"Sugar","InstallLocation":"C:\\Games\\rocketleague"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("b.item"), r#"{"AppName":"Fortnite","InstallLocation":"C:\\FN"}"#).unwrap();
        assert_eq!(epic_installs(&dir), vec![PathBuf::from("C:\\Games\\rocketleague")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arenas() {
        assert_eq!(arena_name("Stadium_P"), "DFH Stadium");
        assert_eq!(arena_name("EuroStadium_Night_P"), "Mannfield");
        assert_eq!(arena_name("CHN_Stadium_P"), "Forbidden Temple");
        assert_eq!(arena_name("Some_New_Arena_P"), "Some New Arena");
    }

    #[test]
    fn plays_of_the_local_player() {
        let mut t = RlTracker::new();
        let me = |g, s, sh| player("Me", 0, 1, g, s, sh, true);
        let mate = player("Mate", 0, 2, 0, 0, 0, true);
        let opp = player("Opp", 1, 3, 0, 0, 0, false);
        let mut out = Vec::new();
        let mut now = 0u64;
        let feed = |t: &mut RlTracker, m: Value, now: &mut u64, out: &mut Vec<GameEvent>| {
            *now += 100;
            out.extend(t.ingest(&m, *now, Some(*now)));
        };
        feed(
            &mut t,
            json!({"Event":"MatchCreated","Data":{"MatchGuid":"G1"}}),
            &mut now,
            &mut out,
        );
        for _ in 0..6 {
            feed(
                &mut t,
                update(vec![me(0, 0, 0), mate.clone(), opp.clone()], "Me", 1, 0, (0, 0)),
                &mut now,
                &mut out,
            );
        }
        feed(
            &mut t,
            json!({"Event":"RoundStarted","Data":{"MatchGuid":"G1"}}),
            &mut now,
            &mut out,
        );
        assert!(t.in_match(now));

        // My goal, refined as an aerial on the same tick; Data arrives as a string here.
        let goal =
            json!({"Scorer":{"Name":"Me","Shortcut":1,"TeamNum":0},"Assister":{"Name":"Mate","Shortcut":2,"TeamNum":0},"GoalSpeed":97.43});
        feed(&mut t, json!({"Event":"GoalScored","Data": goal.to_string()}), &mut now, &mut out);
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"AerialGoal","MainTarget":{"Name":"Me","Shortcut":1,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"Shot","MainTarget":{"Name":"Me","Shortcut":1,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            update(vec![me(1, 0, 1), mate.clone(), opp.clone()], "Me", 1, 0, (1, 0)),
            &mut now,
            &mut out,
        );
        // Save + EpicSave on one tick: one epic save.
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"Save","MainTarget":{"Name":"Me","Shortcut":1,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"EpicSave","MainTarget":{"Name":"Me","Shortcut":1,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            update(vec![me(1, 1, 1), mate.clone(), opp.clone()], "Me", 1, 0, (1, 0)),
            &mut now,
            &mut out,
        );
        // A teammate's save is not mine; the opponent scores; I get demolished by them.
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"Save","MainTarget":{"Name":"Mate","Shortcut":2,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            json!({"Event":"GoalScored","Data":{"Scorer":{"Name":"Opp","Shortcut":3,"TeamNum":1},"GoalSpeed":60.0}}),
            &mut now,
            &mut out,
        );
        feed(
            &mut t,
            json!({"Event":"StatfeedEvent","Data":{"EventName":"Demolish","MainTarget":{"Name":"Opp","Shortcut":3,"TeamNum":1},"SecondaryTarget":{"Name":"Me","Shortcut":1,"TeamNum":0}}}),
            &mut now,
            &mut out,
        );
        let mut demoed = me(1, 1, 1);
        demoed["Attacker"] = json!({"Name":"Opp","Shortcut":3,"TeamNum":1});
        feed(
            &mut t,
            update(vec![demoed, mate.clone(), opp.clone()], "Me", 1, 0, (1, 1)),
            &mut now,
            &mut out,
        );
        // The scoreboard shows a shot the statfeed never reported: counted after the grace period.
        for _ in 0..20 {
            feed(
                &mut t,
                update(vec![me(1, 1, 2), mate.clone(), opp.clone()], "Me", 1, 0, (1, 1)),
                &mut now,
                &mut out,
            );
        }
        feed(
            &mut t,
            json!({"Event":"MatchEnded","Data":{"MatchGuid":"G1","WinnerTeamNum":0}}),
            &mut now,
            &mut out,
        );
        assert!(!t.in_match(now));

        let kinds: Vec<&str> = out
            .iter()
            .map(|e| e.kind.as_str())
            .filter(|k| *k != crate::details::SNAPSHOT_KIND)
            .collect();
        assert_eq!(
            kinds,
            [
                "match_start",
                "goal",
                "shot",
                "epic_save",
                "goal_against",
                "demoed",
                "shot",
                "match_end"
            ],
            "{out:#?}"
        );
        let goal = out.iter().find(|e| e.kind == "goal").unwrap();
        assert_eq!(goal.meta["style"], "Aerial goal");
        assert_eq!(goal.meta["assist"], "Mate");
        let snap = crate::details::latest_snapshot(&out);
        assert_eq!(snap["map"].value, "DFH Stadium");
        assert_eq!(snap["mode"].value, "2v2");
        assert_eq!(snap["score"].value, "1-1");
        assert_eq!(snap["result"].value, "win");
        assert_eq!(snap["goals"].value, "1");
        assert_eq!(snap["saves"].value, "1");
        assert_eq!(snap["player"].value, "Me");
    }

    #[test]
    fn free_play_is_not_a_match_and_identity_carries_over() {
        let mut t = RlTracker::new();
        let solo = json!({ "Event": "UpdateState", "Data": { "Players": [player("Me", 0, 1, 0, 0, 0, true)],
            "Game": { "Teams": [], "Arena": "Park_P", "bReplay": false, "bHasTarget": true, "bHasWinner": false,
                      "Target": { "Name": "Me", "Shortcut": 1, "TeamNum": 0 } } } });
        t.ingest(&solo, 100, None);
        assert!(!t.in_match(100));

        // Match 1 identifies me by the camera; match 2 starts with the camera on someone else
        // (a kickoff spectate) but my platform id still finds me.
        for i in 0..6 {
            t.ingest(
                &update(
                    vec![player("Me", 1, 4, 0, 0, 0, true), player("Opp", 0, 1, 0, 0, 0, false)],
                    "Me",
                    4,
                    1,
                    (0, 0),
                ),
                200 + i,
                None,
            );
        }
        assert!(t.in_match(210));
        t.ingest(&json!({"Event":"MatchDestroyed","Data":{}}), 300, None);
        assert!(!t.in_match(300));
        t.ingest(&json!({"Event":"MatchCreated","Data":{}}), 400, None);
        t.ingest(
            &update(
                vec![player("Me", 0, 7, 0, 0, 0, true), player("Opp", 1, 8, 0, 0, 0, false)],
                "Opp",
                8,
                1,
                (0, 0),
            ),
            500,
            Some(0),
        );
        let out = t.ingest(
            &json!({"Event":"GoalScored","Data":{"Scorer":{"Name":"Me","Shortcut":7,"TeamNum":0}}}),
            600,
            Some(100),
        );
        let mut out = out;
        out.extend(t.ingest(
            &update(
                vec![player("Me", 0, 7, 1, 0, 0, true), player("Opp", 1, 8, 0, 0, 0, false)],
                "Me",
                7,
                0,
                (1, 0),
            ),
            900,
            Some(400),
        ));
        assert!(out.iter().any(|e| e.kind == "goal"), "{out:#?}");
    }
}
