//! SQLite library of recorded matches, their events and clips.
//!
//! The app wraps [`Library`] in a `Mutex`; every method takes `&self`.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::{
    details::MatchDetails,
    events::{EventSource, GameEvent},
};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MatchStatus {
    Recording,
    Processing,
    Ready,
    Error,
}

impl MatchStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchStatus::Recording => "recording",
            MatchStatus::Processing => "processing",
            MatchStatus::Ready => "ready",
            MatchStatus::Error => "error",
        }
    }

    pub fn parse(s: &str) -> MatchStatus {
        match s {
            "recording" => MatchStatus::Recording,
            "processing" => MatchStatus::Processing,
            "ready" => MatchStatus::Ready,
            _ => MatchStatus::Error,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatchRecord {
    pub id: String,
    pub game_id: String,
    pub game_name: String,
    pub path: String,
    pub started_unix_ms: u64,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub status: MatchStatus,
    pub title: Option<String>,
    pub favorite: bool,
    pub thumbnail: Option<String>,
    /// Computed on read; ignored on write.
    #[serde(default)]
    pub event_counts: BTreeMap<String, u32>,
    pub error: Option<String>,
    /// Character, map, K/D/A, result... (see [`crate::details`]).
    #[serde(default)]
    pub details: MatchDetails,
    /// What went wrong or was unusual while recording/processing. Computed on read (stored
    /// facts plus derived ones); written only through [`Library::set_health`].
    #[serde(default)]
    pub health: MatchHealth,
}

/// Recording health facts shown as badges on a match. Every field is optional: unknown is not
/// a problem.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatchHealth {
    /// The recording does not cover the whole match (resumed after a manual stop, or started
    /// mid-match).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The game's local API could not be reached, so plays come from the screen only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub api_unavailable: bool,
    /// Share of frames the encoder skipped or dropped, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_frames: Option<f32>,
    /// Free disk space ran low while recording.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub low_disk: bool,
    /// The encoder that actually recorded, e.g. "NVENC H.264".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
}

impl MatchHealth {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Event kind marking a recording resumed after a manual stop (see the app's session code).
const RESUMED_KIND: &str = "_resumed";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Auto,
    Manual,
    Montage,
}

impl ClipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClipKind::Auto => "auto",
            ClipKind::Manual => "manual",
            ClipKind::Montage => "montage",
        }
    }

    pub fn parse(s: &str) -> ClipKind {
        match s {
            "auto" => ClipKind::Auto,
            "montage" => ClipKind::Montage,
            _ => ClipKind::Manual,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipRecord {
    pub id: String,
    pub match_id: Option<String>,
    pub path: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
    pub kind: ClipKind,
    pub title: String,
    pub event_kinds: Vec<String>,
    pub created_unix_ms: u64,
    pub thumbnail: Option<String>,
    pub size_bytes: u64,
}

pub struct Library {
    conn: Connection,
}

const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE matches (
        id TEXT PRIMARY KEY,
        game_id TEXT NOT NULL,
        game_name TEXT NOT NULL,
        path TEXT NOT NULL,
        started_unix_ms INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL DEFAULT 0,
        size_bytes INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL,
        title TEXT,
        favorite INTEGER NOT NULL DEFAULT 0,
        thumbnail TEXT,
        error TEXT
    );
    CREATE INDEX idx_matches_started ON matches(started_unix_ms);
    CREATE INDEX idx_matches_game ON matches(game_id);
    CREATE TABLE events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        match_id TEXT NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        t_ms INTEGER NOT NULL,
        source_type TEXT NOT NULL,
        source TEXT NOT NULL,
        confidence REAL,
        meta TEXT
    );
    CREATE INDEX idx_events_match ON events(match_id, t_ms);
    CREATE TABLE clips (
        id TEXT PRIMARY KEY,
        match_id TEXT REFERENCES matches(id) ON DELETE SET NULL,
        path TEXT NOT NULL,
        start_ms INTEGER NOT NULL,
        end_ms INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        event_kinds TEXT NOT NULL,
        created_unix_ms INTEGER NOT NULL,
        thumbnail TEXT,
        size_bytes INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_clips_match ON clips(match_id);
    CREATE INDEX idx_clips_created ON clips(created_unix_ms);",
    // v2: match details (JSON object field -> {value, source, confidence})
    "ALTER TABLE matches ADD COLUMN details TEXT",
    // v3: recording health facts (JSON, see MatchHealth)
    "ALTER TABLE matches ADD COLUMN health TEXT",
];

fn source_type(s: &EventSource) -> &'static str {
    match s {
        EventSource::Vision { .. } => "vision",
        EventSource::Api { .. } => "api",
        EventSource::PostMatch { .. } => "postMatch",
        EventSource::Manual => "manual",
    }
}

const MATCH_COLS: &str =
    "id, game_id, game_name, path, started_unix_ms, duration_ms, size_bytes, status, title, favorite, thumbnail, error, details";
const CLIP_COLS: &str =
    "id, match_id, path, start_ms, end_ms, duration_ms, kind, title, event_kinds, created_unix_ms, thumbnail, size_bytes";

fn details_json(d: &MatchDetails) -> Option<String> {
    (!d.is_empty()).then(|| serde_json::to_string(d).unwrap_or_default())
}

fn row_to_match(r: &Row) -> rusqlite::Result<MatchRecord> {
    Ok(MatchRecord {
        id: r.get(0)?,
        game_id: r.get(1)?,
        game_name: r.get(2)?,
        path: r.get(3)?,
        started_unix_ms: r.get::<_, i64>(4)? as u64,
        duration_ms: r.get::<_, i64>(5)? as u64,
        size_bytes: r.get::<_, i64>(6)? as u64,
        status: MatchStatus::parse(&r.get::<_, String>(7)?),
        title: r.get(8)?,
        favorite: r.get::<_, i64>(9)? != 0,
        thumbnail: r.get(10)?,
        event_counts: BTreeMap::new(),
        error: r.get(11)?,
        details: r
            .get::<_, Option<String>>(12)?
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default(),
        health: MatchHealth::default(),
    })
}

fn row_to_clip(r: &Row) -> rusqlite::Result<ClipRecord> {
    let kinds: String = r.get(8)?;
    Ok(ClipRecord {
        id: r.get(0)?,
        match_id: r.get(1)?,
        path: r.get(2)?,
        start_ms: r.get::<_, i64>(3)? as u64,
        end_ms: r.get::<_, i64>(4)? as u64,
        duration_ms: r.get::<_, i64>(5)? as u64,
        kind: ClipKind::parse(&r.get::<_, String>(6)?),
        title: r.get(7)?,
        event_kinds: serde_json::from_str(&kinds).unwrap_or_default(),
        created_unix_ms: r.get::<_, i64>(9)? as u64,
        thumbnail: r.get(10)?,
        size_bytes: r.get::<_, i64>(11)? as u64,
    })
}

fn row_to_event(r: &Row) -> rusqlite::Result<GameEvent> {
    let source: String = r.get(3)?;
    let meta: Option<String> = r.get(5)?;
    Ok(GameEvent {
        id: Some(r.get(0)?),
        kind: r.get(1)?,
        t_ms: r.get::<_, i64>(2)? as u64,
        source: serde_json::from_str(&source).unwrap_or(EventSource::Manual),
        confidence: r.get::<_, Option<f64>>(4)?.map(|c| c as f32),
        meta: meta.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(serde_json::Value::Null),
    })
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening library {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!("BEGIN; {m}; PRAGMA user_version = {}; COMMIT;", i + 1))
                .with_context(|| format!("library migration {}", i + 1))?;
        }
        Ok(Self { conn })
    }

    fn event_counts(&self, match_id: &str) -> Result<BTreeMap<String, u32>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT kind, COUNT(*) FROM events WHERE match_id = ?1 AND kind NOT LIKE '\\_%' ESCAPE '\\' GROUP BY kind")?;
        let rows = stmt.query_map([match_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn with_counts(&self, mut m: MatchRecord) -> Result<MatchRecord> {
        m.event_counts = self.event_counts(&m.id)?;
        m.health = self.health(&m.id)?;
        Ok(m)
    }

    /// Stored health facts plus the ones the events tell (a resumed recording is partial).
    fn health(&self, match_id: &str) -> Result<MatchHealth> {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT health FROM matches WHERE id = ?1", [match_id], |r| r.get(0))
            .optional()?
            .flatten();
        let mut h: MatchHealth = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !h.partial {
            h.partial = self
                .conn
                .prepare_cached("SELECT 1 FROM events WHERE match_id = ?1 AND kind = ?2 LIMIT 1")?
                .exists(params![match_id, RESUMED_KIND])?;
        }
        Ok(h)
    }

    /// Merge `update` into a match's stored health facts (set fields win; unset ones are kept).
    pub fn set_health(&self, match_id: &str, update: &MatchHealth) -> Result<()> {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT health FROM matches WHERE id = ?1", [match_id], |r| r.get(0))
            .optional()?
            .flatten();
        let mut h: MatchHealth = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        h.partial |= update.partial;
        h.api_unavailable |= update.api_unavailable;
        h.low_disk |= update.low_disk;
        if update.dropped_frames.is_some() {
            h.dropped_frames = update.dropped_frames;
        }
        if update.encoder.is_some() {
            h.encoder = update.encoder.clone();
        }
        let json = (!h.is_empty()).then(|| serde_json::to_string(&h)).transpose()?;
        let n = self.conn.execute("UPDATE matches SET health = ?2 WHERE id = ?1", params![match_id, json])?;
        anyhow::ensure!(n == 1, "match {match_id} not found");
        Ok(())
    }

    pub fn insert_match(&self, m: &MatchRecord) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO matches ({MATCH_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
            params![
                m.id,
                m.game_id,
                m.game_name,
                m.path,
                m.started_unix_ms as i64,
                m.duration_ms as i64,
                m.size_bytes as i64,
                m.status.as_str(),
                m.title,
                m.favorite as i64,
                m.thumbnail,
                m.error,
                details_json(&m.details)
            ],
        )?;
        Ok(())
    }

    pub fn update_match(&self, m: &MatchRecord) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE matches SET game_id = ?2, game_name = ?3, path = ?4, started_unix_ms = ?5, duration_ms = ?6,
             size_bytes = ?7, status = ?8, title = ?9, favorite = ?10, thumbnail = ?11, error = ?12, details = ?13 WHERE id = ?1",
            params![
                m.id,
                m.game_id,
                m.game_name,
                m.path,
                m.started_unix_ms as i64,
                m.duration_ms as i64,
                m.size_bytes as i64,
                m.status.as_str(),
                m.title,
                m.favorite as i64,
                m.thumbnail,
                m.error,
                details_json(&m.details)
            ],
        )?;
        anyhow::ensure!(n == 1, "match {} not found", m.id);
        Ok(())
    }

    pub fn get_match(&self, id: &str) -> Result<Option<MatchRecord>> {
        let m = self
            .conn
            .query_row(&format!("SELECT {MATCH_COLS} FROM matches WHERE id = ?1"), [id], row_to_match)
            .optional()?;
        m.map(|m| self.with_counts(m)).transpose()
    }

    pub fn list_matches(&self, game_id: Option<&str>) -> Result<Vec<MatchRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MATCH_COLS} FROM matches WHERE (?1 IS NULL OR game_id = ?1) ORDER BY started_unix_ms DESC"
        ))?;
        let rows: Vec<MatchRecord> = stmt.query_map([game_id], row_to_match)?.collect::<rusqlite::Result<_>>()?;
        rows.into_iter().map(|m| self.with_counts(m)).collect()
    }

    pub fn delete_match(&self, id: &str) -> Result<Option<MatchRecord>> {
        let Some(m) = self.get_match(id)? else { return Ok(None) };
        self.conn.execute("DELETE FROM events WHERE match_id = ?1", [id])?;
        self.conn.execute("UPDATE clips SET match_id = NULL WHERE match_id = ?1", [id])?;
        self.conn.execute("DELETE FROM matches WHERE id = ?1", [id])?;
        Ok(Some(m))
    }

    pub fn add_events(&self, match_id: &str, events: &[GameEvent]) -> Result<Vec<GameEvent>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut out = Vec::with_capacity(events.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO events (match_id, kind, t_ms, source_type, source, confidence, meta) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for e in events {
                let meta = if e.meta.is_null() {
                    None
                } else {
                    Some(serde_json::to_string(&e.meta)?)
                };
                stmt.execute(params![
                    match_id,
                    e.kind,
                    e.t_ms as i64,
                    source_type(&e.source),
                    serde_json::to_string(&e.source)?,
                    e.confidence.map(|c| c as f64),
                    meta
                ])?;
                let mut ev = e.clone();
                ev.id = Some(tx.last_insert_rowid());
                out.push(ev);
            }
        }
        tx.commit()?;
        Ok(out)
    }

    pub fn list_events(&self, match_id: &str) -> Result<Vec<GameEvent>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, kind, t_ms, source, confidence, meta FROM events WHERE match_id = ?1 ORDER BY t_ms, id")?;
        let rows = stmt.query_map([match_id], row_to_event)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn delete_event(&self, match_id: &str, event_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM events WHERE match_id = ?1 AND id = ?2", params![match_id, event_id])?;
        Ok(())
    }

    /// Delete vision and post-match events (they are recomputed on rescan); keep API and manual ones.
    pub fn clear_derived_events(&self, match_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM events WHERE match_id = ?1 AND source_type IN ('vision', 'postMatch')",
            [match_id],
        )?;
        Ok(())
    }

    pub fn insert_clip(&self, c: &ClipRecord) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO clips ({CLIP_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                c.id,
                c.match_id,
                c.path,
                c.start_ms as i64,
                c.end_ms as i64,
                c.duration_ms as i64,
                c.kind.as_str(),
                c.title,
                serde_json::to_string(&c.event_kinds)?,
                c.created_unix_ms as i64,
                c.thumbnail,
                c.size_bytes as i64
            ],
        )?;
        Ok(())
    }

    pub fn get_clip(&self, id: &str) -> Result<Option<ClipRecord>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {CLIP_COLS} FROM clips WHERE id = ?1"), [id], row_to_clip)
            .optional()?)
    }

    pub fn list_clips(&self, match_id: Option<&str>) -> Result<Vec<ClipRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CLIP_COLS} FROM clips WHERE (?1 IS NULL OR match_id = ?1) ORDER BY created_unix_ms DESC, rowid DESC"
        ))?;
        let rows = stmt.query_map([match_id], row_to_clip)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn update_clip_title(&self, id: &str, title: &str) -> Result<Option<ClipRecord>> {
        self.conn.execute("UPDATE clips SET title = ?2 WHERE id = ?1", params![id, title])?;
        self.get_clip(id)
    }

    pub fn delete_clip(&self, id: &str) -> Result<Option<ClipRecord>> {
        let c = self.get_clip(id)?;
        if c.is_some() {
            self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        }
        Ok(c)
    }

    /// At startup: anything still marked recording/processing was interrupted by a crash or shutdown.
    pub fn mark_interrupted_recordings(&self) -> Result<Vec<MatchRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MATCH_COLS} FROM matches WHERE status IN ('recording', 'processing') ORDER BY started_unix_ms"
        ))?;
        let rows: Vec<MatchRecord> = stmt.query_map([], row_to_match)?.collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for mut m in rows {
            m.status = MatchStatus::Error;
            m.error = Some("interrupted".into());
            self.update_match(&m)?;
            out.push(self.with_counts(m)?);
        }
        Ok(out)
    }

    /// Candidates for storage-limit cleanup: non-favourite, finished matches, oldest first.
    pub fn oldest_deletable_matches(&self) -> Result<Vec<MatchRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MATCH_COLS} FROM matches WHERE favorite = 0 AND status IN ('ready', 'error') ORDER BY started_unix_ms ASC"
        ))?;
        let rows: Vec<MatchRecord> = stmt.query_map([], row_to_match)?.collect::<rusqlite::Result<_>>()?;
        rows.into_iter().map(|m| self.with_counts(m)).collect()
    }

    /// (match count, clip count, total bytes of matches + clips) as recorded in the library.
    pub fn storage_stats(&self) -> Result<(u64, u64, u64)> {
        let (mc, mb): (i64, i64) = self
            .conn
            .query_row("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM matches", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let (cc, cb): (i64, i64) = self
            .conn
            .query_row("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM clips", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        Ok((mc as u64, cc as u64, (mb + cb) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::kind;

    fn m(id: &str, started: u64, status: MatchStatus) -> MatchRecord {
        MatchRecord {
            id: id.into(),
            game_id: "valorant".into(),
            game_name: "VALORANT".into(),
            path: format!("C:/v/{id}.mp4"),
            started_unix_ms: started,
            duration_ms: 1000,
            size_bytes: 100,
            status,
            title: None,
            favorite: false,
            thumbnail: None,
            event_counts: BTreeMap::new(),
            error: None,
            details: Default::default(),
            health: Default::default(),
        }
    }

    fn clip(id: &str, match_id: Option<&str>, created: u64) -> ClipRecord {
        ClipRecord {
            id: id.into(),
            match_id: match_id.map(Into::into),
            path: format!("C:/c/{id}.mp4"),
            start_ms: 0,
            end_ms: 8000,
            duration_ms: 8000,
            kind: ClipKind::Auto,
            title: "Kill".into(),
            event_kinds: vec!["kill".into()],
            created_unix_ms: created,
            thumbnail: None,
            size_bytes: 50,
        }
    }

    #[test]
    fn matches_events_and_clips() {
        let lib = Library::open_in_memory().unwrap();
        lib.insert_match(&m("a", 1, MatchStatus::Ready)).unwrap();
        lib.insert_match(&m("b", 2, MatchStatus::Recording)).unwrap();

        let evs = lib
            .add_events(
                "a",
                &[
                    GameEvent::new(kind::KILL, 500, EventSource::Vision { detector: "k".into() }).with_confidence(0.9),
                    GameEvent::new(kind::KILL, 100, EventSource::Api { name: "x".into() }).with_meta(serde_json::json!({"victim": "v"})),
                    GameEvent::new(kind::BOOKMARK, 300, EventSource::Manual),
                    GameEvent::new(kind::DEATH, 400, EventSource::PostMatch { processor: "p".into() }),
                ],
            )
            .unwrap();
        assert!(evs.iter().all(|e| e.id.is_some()));

        let list = lib.list_matches(None).unwrap();
        assert_eq!(list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(list[1].event_counts.get("kill"), Some(&2));
        assert_eq!(lib.list_matches(Some("other")).unwrap().len(), 0);

        let events = lib.list_events("a").unwrap();
        assert_eq!(events.iter().map(|e| e.t_ms).collect::<Vec<_>>(), vec![100, 300, 400, 500]);
        assert_eq!(events[0].meta["victim"], "v");
        assert_eq!(events[3].confidence, Some(0.9));

        lib.clear_derived_events("a").unwrap();
        let kinds: Vec<_> = lib.list_events("a").unwrap().into_iter().map(|e| e.source).collect();
        assert_eq!(kinds, vec![EventSource::Api { name: "x".into() }, EventSource::Manual]);

        let id = lib.list_events("a").unwrap()[1].id.unwrap();
        lib.delete_event("a", id).unwrap();
        assert_eq!(lib.list_events("a").unwrap().len(), 1);

        lib.insert_clip(&clip("c1", Some("a"), 10)).unwrap();
        lib.insert_clip(&clip("c2", None, 20)).unwrap();
        assert_eq!(lib.list_clips(None).unwrap()[0].id, "c2");
        assert_eq!(lib.list_clips(Some("a")).unwrap().len(), 1);
        assert_eq!(lib.update_clip_title("c1", "Ace").unwrap().unwrap().title, "Ace");

        let deleted = lib.delete_match("a").unwrap().unwrap();
        assert_eq!(deleted.id, "a");
        assert!(lib.get_match("a").unwrap().is_none());
        let c1 = lib.get_clip("c1").unwrap().unwrap();
        assert_eq!(c1.match_id, None, "clip survives with match_id NULL");
        assert_eq!(lib.list_events("a").unwrap().len(), 0);
        assert!(lib.delete_clip("c2").unwrap().is_some());
        assert!(lib.delete_clip("c2").unwrap().is_none());
    }

    #[test]
    fn update_and_interrupted_and_deletable() {
        let lib = Library::open_in_memory().unwrap();
        lib.insert_match(&m("a", 1, MatchStatus::Ready)).unwrap();
        lib.insert_match(&m("b", 2, MatchStatus::Processing)).unwrap();
        let mut fav = m("c", 0, MatchStatus::Ready);
        fav.favorite = true;
        lib.insert_match(&fav).unwrap();

        let mut a = lib.get_match("a").unwrap().unwrap();
        a.title = Some("Clutch".into());
        lib.update_match(&a).unwrap();
        assert_eq!(lib.get_match("a").unwrap().unwrap().title.as_deref(), Some("Clutch"));
        assert!(lib.update_match(&m("zzz", 0, MatchStatus::Ready)).is_err());

        let interrupted = lib.mark_interrupted_recordings().unwrap();
        assert_eq!(interrupted.len(), 1);
        let b = lib.get_match("b").unwrap().unwrap();
        assert_eq!(b.status, MatchStatus::Error);
        assert_eq!(b.error.as_deref(), Some("interrupted"));

        let del: Vec<_> = lib.oldest_deletable_matches().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(del, vec!["a", "b"]);
        assert_eq!(lib.storage_stats().unwrap(), (3, 0, 300));
    }

    #[test]
    fn health_is_stored_merged_and_derived() {
        let lib = Library::open_in_memory().unwrap();
        lib.insert_match(&m("a", 1, MatchStatus::Ready)).unwrap();
        assert!(lib.get_match("a").unwrap().unwrap().health.is_empty());
        lib.set_health(
            "a",
            &MatchHealth {
                dropped_frames: Some(0.04),
                encoder: Some("NVENC H.264".into()),
                ..Default::default()
            },
        )
        .unwrap();
        lib.set_health("a", &MatchHealth { low_disk: true, ..Default::default() }).unwrap();
        let h = lib.get_match("a").unwrap().unwrap().health;
        assert_eq!(h.dropped_frames, Some(0.04));
        assert_eq!(h.encoder.as_deref(), Some("NVENC H.264"));
        assert!(h.low_disk && !h.partial);
        // A resumed recording is partial.
        lib.add_events("a", &[GameEvent::new(RESUMED_KIND, 0, EventSource::Manual)]).unwrap();
        assert!(lib.list_matches(None).unwrap()[0].health.partial);
        assert!(lib.set_health("zzz", &MatchHealth::default()).is_err());
        // Serialized without the unset fields.
        let v = serde_json::to_value(&lib.get_match("a").unwrap().unwrap().health).unwrap();
        assert_eq!(v, serde_json::json!({"partial": true, "droppedFrames": 0.04f32, "lowDisk": true, "encoder": "NVENC H.264"}));
    }

    #[test]
    fn reopen_file_keeps_data() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("lib/library.db");
        {
            let lib = Library::open(&p).unwrap();
            lib.insert_match(&m("a", 1, MatchStatus::Ready)).unwrap();
        }
        let lib = Library::open(&p).unwrap();
        assert!(lib.get_match("a").unwrap().is_some());
    }

    #[test]
    fn serde_shapes() {
        let s = serde_json::to_value(m("a", 1, MatchStatus::Ready)).unwrap();
        assert_eq!(s["status"], "ready");
        assert!(s.get("startedUnixMs").is_some());
        let c = serde_json::to_value(clip("c", None, 1)).unwrap();
        assert_eq!(c["kind"], "auto");
        assert!(c["matchId"].is_null());
    }
}
