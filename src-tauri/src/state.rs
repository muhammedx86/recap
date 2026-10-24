//! Shared application state and the types the UI sees.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex, RwLock},
    time::Instant,
};

use recap_core::{
    cs2::Cs2Tracker,
    events::GameEvent,
    ffmpeg::Ffmpeg,
    games::{load_games, ApiSource, CaptureMode, DetectorDef, LoadedGame, ReaderDef},
    library::Library,
    settings::Settings,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{paths::Paths, recorder::Recorder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecState {
    Idle,
    Starting,
    Recording,
    Stopping,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub state: RecState,
    pub match_id: Option<String>,
    pub game_id: Option<String>,
    pub game_name: Option<String>,
    pub started_unix_ms: Option<u64>,
    pub encoder: Option<String>,
    pub capture: Option<CaptureMode>,
    pub bookmarks: u32,
    pub message: Option<String>,
}

impl RecorderStatus {
    pub fn idle(message: Option<String>) -> Self {
        Self {
            state: RecState::Idle,
            match_id: None,
            game_id: None,
            game_name: None,
            started_unix_ms: None,
            encoder: None,
            capture: None,
            bookmarks: 0,
            message,
        }
    }
}

/// One recording in progress.
pub struct Session {
    pub match_id: String,
    /// None for manual recordings.
    pub game: Option<LoadedGame>,
    pub pid: Option<u32>,
    pub path: PathBuf,
    pub requested: Instant,
    pub started: Option<Instant>,
    pub started_unix_ms: u64,
    /// Tells API pollers to stop.
    pub stop_flag: Arc<AtomicBool>,
    pub bookmarks: u32,
    pub stopping: bool,
    pub cs2: Option<Cs2Tracker>,
}

impl Session {
    /// Milliseconds since the recording started (or since it was requested, before the first frame).
    pub fn elapsed_ms(&self) -> u64 {
        self.started.unwrap_or(self.requested).elapsed().as_millis() as u64
    }

    pub fn game_id(&self) -> String {
        self.game.as_ref().map(|g| g.def.id.clone()).unwrap_or_else(|| "manual".into())
    }

    pub fn game_name(&self) -> String {
        self.game
            .as_ref()
            .map(|g| g.def.name.clone())
            .unwrap_or_else(|| "Manual recording".into())
    }
}

pub struct AppState {
    pub paths: Paths,
    pub settings: RwLock<Settings>,
    pub library: Mutex<Library>,
    pub games: RwLock<Vec<LoadedGame>>,
    pub ff: Ffmpeg,
    pub recorder: Recorder,
    pub session: Mutex<Option<Session>>,
    pub status: RwLock<RecorderStatus>,
    /// Matches currently being processed.
    pub processing: Mutex<HashSet<String>>,
    /// CS2 GSI auth token.
    pub gsi_token: String,
}

impl AppState {
    pub fn reload_games(&self) {
        let games = load_games(&self.paths.game_roots());
        log::info!("loaded {} games", games.len());
        *self.games.write().unwrap() = games;
    }

    pub fn game(&self, id: &str) -> Option<LoadedGame> {
        self.games.read().unwrap().iter().find(|g| g.def.id == id).cloned()
    }

    pub fn recordings_dir(&self) -> PathBuf {
        let s = self.settings.read().unwrap();
        if s.recordings_dir.trim().is_empty() {
            crate::paths::default_recordings_dir()
        } else {
            PathBuf::from(s.recordings_dir.trim())
        }
    }

    pub fn game_info(&self, g: &LoadedGame) -> GameInfo {
        let enabled = self.settings.read().unwrap().game_enabled(&g.def.id);
        GameInfo::from_loaded(g, enabled)
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

/// Recompute status from the session and broadcast it.
pub fn refresh_status(app: &AppHandle, message: Option<String>) {
    let st = state(app);
    let status = {
        let session = st.session.lock().unwrap();
        let prev = st.status.read().unwrap().clone();
        match session.as_ref() {
            None => {
                let state = if st.recorder.available() {
                    RecState::Idle
                } else {
                    RecState::Unavailable
                };
                let msg = message.or_else(|| (!st.recorder.available()).then(|| "Recorder not installed".to_string()));
                RecorderStatus {
                    state,
                    ..RecorderStatus::idle(msg)
                }
            }
            Some(s) => RecorderStatus {
                state: if s.stopping {
                    RecState::Stopping
                } else if s.started.is_some() {
                    RecState::Recording
                } else {
                    RecState::Starting
                },
                match_id: Some(s.match_id.clone()),
                game_id: Some(s.game_id()),
                game_name: Some(s.game_name()),
                started_unix_ms: s.started.map(|_| s.started_unix_ms),
                encoder: if prev.match_id.as_deref() == Some(&s.match_id) {
                    prev.encoder.clone()
                } else {
                    None
                },
                capture: if prev.match_id.as_deref() == Some(&s.match_id) {
                    prev.capture
                } else {
                    None
                },
                bookmarks: s.bookmarks,
                message: message.or(prev.message.filter(|_| prev.match_id.as_deref() == Some(&s.match_id))),
            },
        }
    };
    *st.status.write().unwrap() = status.clone();
    let _ = app.emit("status-changed", status);
}

/// Store live events (API, bookmarks) for the active match.
pub fn add_live_events(app: &AppHandle, match_id: &str, events: Vec<GameEvent>) {
    if events.is_empty() {
        return;
    }
    let st = state(app);
    let res = st.library.lock().unwrap().add_events(match_id, &events);
    match res {
        Ok(_) => {
            let _ = app.emit("library-changed", serde_json::json!({ "matchId": match_id }));
        }
        Err(e) => log::error!("storing live events: {e:#}"),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectorInfo {
    #[serde(flatten)]
    pub def: DetectorDef,
    pub ready: bool,
    pub user: bool,
    pub template_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    pub id: String,
    pub name: String,
    pub processes: Vec<String>,
    pub capture: CaptureMode,
    pub api: Option<ApiSource>,
    pub prefer_api: bool,
    pub auto_clip_events: Vec<String>,
    pub color: Option<String>,
    pub enabled: bool,
    pub user_modified: bool,
    pub detectors: Vec<DetectorInfo>,
    pub has_post_match: bool,
    pub has_rounds: bool,
    /// "api" | "detectors" | "process"
    pub match_detection: &'static str,
    pub event_types: Vec<recap_core::events::EventTypeDef>,
    pub readers: Vec<ReaderInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderInfo {
    #[serde(flatten)]
    pub def: ReaderDef,
    pub ready: bool,
    pub user: bool,
    /// Reference image paths per choice option value.
    pub option_urls: std::collections::BTreeMap<String, Vec<String>>,
}

impl GameInfo {
    pub fn from_loaded(g: &LoadedGame, enabled: bool) -> Self {
        Self {
            id: g.def.id.clone(),
            name: g.def.name.clone(),
            processes: g.def.processes.clone(),
            capture: g.def.capture,
            api: g.def.api,
            prefer_api: g.def.prefer_api,
            auto_clip_events: g.def.auto_clip.events.clone(),
            color: g.def.color.clone(),
            enabled,
            user_modified: g.user_modified,
            detectors: g
                .detectors
                .iter()
                .map(|d| DetectorInfo {
                    ready: d.def.is_ready() && d.template_paths().iter().all(|p| p.exists()),
                    user: d.user,
                    template_urls: d.template_paths().iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                    def: d.def.clone(),
                })
                .collect(),
            has_post_match: !g.def.post_match.is_empty(),
            has_rounds: g.has_rounds(),
            match_detection: if g.def.api.is_some_and(|a| a.detects_matches()) {
                "api"
            } else if g
                .detectors
                .iter()
                .any(|d| d.def.event == recap_core::events::kind::MATCH_START || d.def.event == recap_core::events::kind::MATCH_END)
            {
                "detectors"
            } else {
                "process"
            },
            event_types: g.event_types(),
            readers: g
                .readers
                .iter()
                .map(|r| ReaderInfo {
                    ready: r.def.is_ready(),
                    user: r.user,
                    option_urls: r
                        .def
                        .options
                        .iter()
                        .map(|o| {
                            let urls = o
                                .templates
                                .iter()
                                .map(|t| r.base_dir.join(t).to_string_lossy().into_owned())
                                .collect();
                            (o.value.clone(), urls)
                        })
                        .collect(),
                    def: r.def.clone(),
                })
                .collect(),
        }
    }
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
