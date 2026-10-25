//! Tauri commands. Signatures follow docs/API.md exactly.

use std::{fs, path::Path};

use recap_core::{
    events::{builtin_event_types, EventSource, EventTypeDef, GameEvent},
    games::{
        delete_user_detector, delete_user_reader, remove_event_type, save_user_game, upsert_event_type, ApiSource, AutoClipDef,
        CaptureMode, DetectorDef, GameDef, ReaderDef, Region,
    },
    library::{ClipKind, ClipRecord, MatchRecord},
    settings::Settings,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::{
    hotkeys, processing, session,
    state::{refresh_status, AppState, GameInfo, RecorderStatus},
    studio::{self, FrameReadResult, FrameResult, FrameSource, OptionCrop, ReaderTestResult, TestResult, VideoInfoResult},
};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

fn changed(app: &AppHandle, match_id: Option<&str>) {
    let payload = match match_id {
        Some(id) => serde_json::json!({ "matchId": id }),
        None => serde_json::json!({}),
    };
    let _ = app.emit("library-changed", payload);
}

#[tauri::command]
pub fn get_status(st: State<'_, AppState>) -> RecorderStatus {
    st.status.read().unwrap().clone()
}

#[tauri::command]
pub fn list_matches(st: State<'_, AppState>, game_id: Option<String>) -> CmdResult<Vec<MatchRecord>> {
    st.library.lock().unwrap().list_matches(game_id.as_deref()).map_err(err)
}

#[derive(Serialize)]
pub struct MatchDetail {
    #[serde(rename = "match")]
    m: MatchRecord,
    events: Vec<GameEvent>,
    clips: Vec<ClipRecord>,
}

#[tauri::command]
pub fn get_match(st: State<'_, AppState>, id: String) -> CmdResult<MatchDetail> {
    let lib = st.library.lock().unwrap();
    let m = lib.get_match(&id).map_err(err)?.ok_or("Recording not found")?;
    Ok(MatchDetail {
        events: lib.list_events(&id).map_err(err)?,
        clips: lib.list_clips(Some(&id)).map_err(err)?,
        m,
    })
}

#[tauri::command]
pub fn update_match(
    app: AppHandle,
    st: State<'_, AppState>,
    id: String,
    title: Option<String>,
    favorite: Option<bool>,
) -> CmdResult<MatchRecord> {
    let lib = st.library.lock().unwrap();
    let mut m = lib.get_match(&id).map_err(err)?.ok_or("Recording not found")?;
    // An empty title clears it (JSON null cannot be told apart from "not provided").
    if let Some(t) = title {
        let t = t.trim().to_string();
        m.title = (!t.is_empty()).then_some(t);
    }
    if let Some(f) = favorite {
        m.favorite = f;
    }
    lib.update_match(&m).map_err(err)?;
    let m = lib.get_match(&id).map_err(err)?.ok_or("Recording not found")?;
    drop(lib);
    changed(&app, Some(&id));
    Ok(m)
}

/// Set (or clear, with an empty value) one match detail by hand. Typed values are never
/// replaced by rescans.
#[tauri::command]
pub fn update_match_detail(app: AppHandle, st: State<'_, AppState>, id: String, field: String, value: String) -> CmdResult<MatchRecord> {
    let field = field.trim().to_string();
    if field.is_empty() {
        return Err("Missing field".into());
    }
    let lib = st.library.lock().unwrap();
    let mut m = lib.get_match(&id).map_err(err)?.ok_or("Recording not found")?;
    recap_core::details::offer(
        &mut m.details,
        &field,
        recap_core::details::DetailValue::new(value, recap_core::details::DetailSource::Manual),
    );
    lib.update_match(&m).map_err(err)?;
    let m = lib.get_match(&id).map_err(err)?.ok_or("Recording not found")?;
    drop(lib);
    changed(&app, Some(&id));
    Ok(m)
}

#[tauri::command]
pub fn delete_match(app: AppHandle, st: State<'_, AppState>, id: String, delete_files: bool) -> CmdResult<()> {
    if st.session.lock().unwrap().as_ref().is_some_and(|s| s.match_id == id) {
        return Err("This match is still recording".into());
    }
    if st.processing.lock().unwrap().contains(&id) {
        return Err("This match is still processing".into());
    }
    let removed = st.library.lock().unwrap().delete_match(&id).map_err(err)?;
    if let Some(m) = removed {
        if let Some(t) = &m.thumbnail {
            let _ = fs::remove_file(t);
        }
        if delete_files {
            fs::remove_file(&m.path)
                .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
                .map_err(err)?;
        }
    }
    changed(&app, Some(&id));
    Ok(())
}

#[tauri::command]
pub fn add_event(app: AppHandle, st: State<'_, AppState>, match_id: String, kind: String, t_ms: u64) -> CmdResult<GameEvent> {
    let kind = kind.trim().to_lowercase();
    if kind.is_empty() {
        return Err("Event kind is required".into());
    }
    let ev = GameEvent::new(kind, t_ms, EventSource::Manual);
    let mut stored = st.library.lock().unwrap().add_events(&match_id, &[ev]).map_err(err)?;
    changed(&app, Some(&match_id));
    stored.pop().ok_or_else(|| "Event not stored".into())
}

#[tauri::command]
pub fn delete_event(app: AppHandle, st: State<'_, AppState>, match_id: String, event_id: i64) -> CmdResult<()> {
    st.library.lock().unwrap().delete_event(&match_id, event_id).map_err(err)?;
    changed(&app, Some(&match_id));
    Ok(())
}

#[tauri::command]
pub fn rescan_match(app: AppHandle, st: State<'_, AppState>, id: String) -> CmdResult<()> {
    if st.session.lock().unwrap().as_ref().is_some_and(|s| s.match_id == id) {
        return Err("This match is still recording".into());
    }
    processing::spawn(app, id, true);
    Ok(())
}

#[tauri::command]
pub fn list_clips(st: State<'_, AppState>, match_id: Option<String>) -> CmdResult<Vec<ClipRecord>> {
    st.library.lock().unwrap().list_clips(match_id.as_deref()).map_err(err)
}

#[tauri::command]
pub async fn create_clip(app: AppHandle, match_id: String, start_ms: u64, end_ms: u64, title: Option<String>) -> CmdResult<ClipRecord> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        let (m, events) = {
            let lib = st.library.lock().unwrap();
            let m = lib.get_match(&match_id).map_err(err)?.ok_or("Recording not found")?;
            let ev = lib.list_events(&match_id).map_err(err)?;
            (m, ev)
        };
        let end_ms = if m.duration_ms > 0 { end_ms.min(m.duration_ms) } else { end_ms };
        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("{} clip", m.game_name));
        let clip = processing::make_clip(&st, &m, start_ms, end_ms, &title, ClipKind::Manual, &events).map_err(err)?;
        changed(&app, Some(&match_id));
        Ok(clip)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn create_montage(app: AppHandle, clip_ids: Vec<String>, title: String) -> CmdResult<ClipRecord> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        let clip = processing::make_montage(&st, &clip_ids, &title).map_err(err)?;
        changed(&app, None);
        Ok(clip)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn update_clip(app: AppHandle, st: State<'_, AppState>, id: String, title: String) -> CmdResult<ClipRecord> {
    let c = st
        .library
        .lock()
        .unwrap()
        .update_clip_title(&id, title.trim())
        .map_err(err)?
        .ok_or("Clip not found")?;
    changed(&app, c.match_id.as_deref());
    Ok(c)
}

#[tauri::command]
pub fn delete_clip(app: AppHandle, st: State<'_, AppState>, id: String, delete_file: bool) -> CmdResult<()> {
    let c = st.library.lock().unwrap().delete_clip(&id).map_err(err)?;
    if let Some(c) = c {
        if let Some(t) = &c.thumbnail {
            let _ = fs::remove_file(t);
        }
        if delete_file {
            let _ = fs::remove_file(&c.path);
        }
        changed(&app, c.match_id.as_deref());
    }
    Ok(())
}

#[tauri::command]
pub fn list_games(st: State<'_, AppState>) -> Vec<GameInfo> {
    st.games.read().unwrap().iter().map(|g| st.game_info(g)).collect()
}

/// Rocket League's Stats API on this computer (installs found, switched on, streaming).
#[tauri::command]
pub fn rocket_league_api_status() -> crate::sources::RlApiStatus {
    crate::sources::rocket_league_status()
}

/// Switch Rocket League's Stats API on, asking for administrator rights if the install is in a
/// protected folder.
#[tauri::command]
pub async fn enable_rocket_league_api() -> crate::sources::RlApiStatus {
    tauri::async_runtime::spawn_blocking(|| crate::sources::enable_rocket_league_api(true))
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn set_game_enabled(st: State<'_, AppState>, game_id: String, enabled: bool) -> CmdResult<GameInfo> {
    let game = st.game(&game_id).ok_or("Unknown game")?;
    {
        let mut s = st.settings.write().unwrap();
        s.games_enabled.insert(game_id, enabled);
        s.save(&st.paths.settings_file).map_err(err)?;
    }
    Ok(st.game_info(&game))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInput {
    id: String,
    name: String,
    processes: Vec<String>,
    capture: CaptureMode,
    api: Option<ApiSource>,
    prefer_api: bool,
    auto_clip_events: Vec<String>,
    color: Option<String>,
}

#[tauri::command]
pub fn save_game(st: State<'_, AppState>, game: GameInput) -> CmdResult<GameInfo> {
    let id = game.id.trim().to_lowercase();
    let existing = st.game(&id);
    let def = GameDef {
        id,
        name: game.name.trim().to_string(),
        processes: game
            .processes
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        capture: game.capture,
        api: game.api,
        prefer_api: game.prefer_api,
        auto_clip: AutoClipDef {
            events: if game.auto_clip_events.is_empty() {
                AutoClipDef::default().events
            } else {
                game.auto_clip_events
            },
            timing: existing.as_ref().map(|g| g.def.auto_clip.timing.clone()).unwrap_or_default(),
        },
        post_match: existing.as_ref().map(|g| g.def.post_match.clone()).unwrap_or_default(),
        event_types: existing.as_ref().map(|g| g.def.event_types.clone()).unwrap_or_default(),
        rounds: existing.as_ref().and_then(|g| g.def.rounds.clone()),
        exclusions: existing.map(|g| g.def.exclusions).unwrap_or_default(),
        color: game.color,
    };
    if def.name.is_empty() {
        return Err("Name is required".into());
    }
    save_user_game(&st.paths.user_games, &def).map_err(err)?;
    st.reload_games();
    st.game(&def.id)
        .map(|g| st.game_info(&g))
        .ok_or_else(|| "Game was saved but could not be loaded".into())
}

#[tauri::command]
pub fn list_event_types(st: State<'_, AppState>) -> Vec<EventTypeDef> {
    let mut all = builtin_event_types();
    for g in st.games.read().unwrap().iter() {
        for t in &g.def.event_types {
            if !all.iter().any(|x| x.id == t.id) {
                all.push(t.clone());
            }
        }
    }
    all
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTypeInput {
    #[serde(default)]
    id: Option<String>,
    label: String,
    icon: String,
    #[serde(default)]
    color: Option<String>,
}

#[tauri::command]
pub fn save_event_type(st: State<'_, AppState>, game_id: String, event_type: EventTypeInput) -> CmdResult<GameInfo> {
    let game = st.game(&game_id).ok_or("Unknown game")?;
    let t = EventTypeDef {
        id: event_type.id.unwrap_or_default().trim().to_string(),
        label: event_type.label,
        icon: event_type.icon,
        color: event_type.color.unwrap_or_default(),
        builtin: false,
    };
    upsert_event_type(&st.paths.user_games, &game.def, t).map_err(err)?;
    st.reload_games();
    st.game(&game_id).map(|g| st.game_info(&g)).ok_or_else(|| "Unknown game".into())
}

#[tauri::command]
pub fn delete_event_type(st: State<'_, AppState>, game_id: String, event_type_id: String) -> CmdResult<GameInfo> {
    let game = st.game(&game_id).ok_or("Unknown game")?;
    remove_event_type(&st.paths.user_games, &game, &event_type_id).map_err(err)?;
    st.reload_games();
    st.game(&game_id).map(|g| st.game_info(&g)).ok_or_else(|| "Unknown game".into())
}

#[tauri::command]
pub fn get_settings(st: State<'_, AppState>) -> Settings {
    st.settings.read().unwrap().clone()
}

#[tauri::command]
pub fn update_settings(app: AppHandle, st: State<'_, AppState>, settings: Settings) -> CmdResult<Settings> {
    if settings.fps == 0 || settings.fps > 240 {
        return Err("FPS must be between 1 and 240".into());
    }
    if settings.bitrate_kbps < 500 {
        return Err("Bitrate must be at least 500 kbps".into());
    }
    if !["top-right", "top-left", "bottom-right", "bottom-left"].contains(&settings.notification_corner.as_str()) {
        return Err("Unknown notification corner".into());
    }
    if !settings.recordings_dir.trim().is_empty() {
        fs::create_dir_all(settings.recordings_dir.trim()).map_err(|e| format!("Cannot use that recordings folder: {e}"))?;
    }
    let previous = std::mem::replace(&mut *st.settings.write().unwrap(), settings.clone());
    let hotkeys_changed =
        previous.bookmark_hotkey != settings.bookmark_hotkey || previous.manual_record_hotkey != settings.manual_record_hotkey;
    if hotkeys_changed {
        if let Err(e) = hotkeys::register(&app) {
            // Roll back so a bad hotkey never gets saved.
            *st.settings.write().unwrap() = previous;
            let _ = hotkeys::register(&app);
            return Err(err(e));
        }
    }
    st.settings.read().unwrap().save(&st.paths.settings_file).map_err(err)?;
    if previous.gpu_preference != settings.gpu_preference {
        crate::arming::gpu_preference::apply(&st.paths.recorder_exe, settings.gpu_preference);
    }
    if previous.launch_at_startup != settings.launch_at_startup {
        crate::startup::apply(&app, settings.launch_at_startup);
    }
    let app2 = app.clone();
    std::thread::spawn(move || processing::enforce_storage_limit(&app2));
    Ok(settings)
}

#[tauri::command]
pub async fn list_encoders(app: AppHandle) -> CmdResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || crate::state::state(&app).recorder.list_encoders().map_err(err))
        .await
        .map_err(err)?
}

#[derive(Serialize)]
pub struct AudioInput {
    id: String,
    name: String,
}

#[tauri::command]
pub fn list_audio_inputs() -> Vec<AudioInput> {
    // The recorder records the system default microphone ("default"). Device enumeration
    // can be added later through the recorder protocol.
    vec![AudioInput {
        id: "default".into(),
        name: "System default microphone".into(),
    }]
}

#[tauri::command]
pub fn start_manual_recording(app: AppHandle, st: State<'_, AppState>) -> CmdResult<RecorderStatus> {
    session::start(&app, None, None).map_err(err)?;
    Ok(st.status.read().unwrap().clone())
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, st: State<'_, AppState>) -> CmdResult<RecorderStatus> {
    // In a game: skip this match only; the next one records again.
    crate::monitor::on_manual_stop(&app);
    session::stop(&app).map_err(err)?;
    Ok(st.status.read().unwrap().clone())
}

#[tauri::command]
pub fn add_bookmark(app: AppHandle) {
    session::bookmark(&app);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    recordings_dir: String,
    used_bytes: u64,
    free_bytes: u64,
    match_count: usize,
    clip_count: usize,
}

#[tauri::command]
pub fn get_storage(st: State<'_, AppState>) -> CmdResult<StorageInfo> {
    let dir = st.recordings_dir();
    let lib = st.library.lock().unwrap();
    let matches = lib.list_matches(None).map_err(err)?;
    let clips = lib.list_clips(None).map_err(err)?;
    drop(lib);
    let used = matches.iter().map(|m| m.size_bytes).sum::<u64>() + clips.iter().map(|c| c.size_bytes).sum::<u64>();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let probe = if dir.exists() {
        dir.clone()
    } else {
        dir.ancestors().find(|p| p.exists()).map(Path::to_path_buf).unwrap_or_default()
    };
    let free = disks
        .list()
        .iter()
        .filter(|d| probe.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
        .unwrap_or(0);
    Ok(StorageInfo {
        recordings_dir: dir.to_string_lossy().into_owned(),
        used_bytes: used,
        free_bytes: free,
        match_count: matches.len(),
        clip_count: clips.len(),
    })
}

#[tauri::command]
pub async fn studio_load_frame(app: AppHandle, source: FrameSource) -> CmdResult<FrameResult> {
    tauri::async_runtime::spawn_blocking(move || studio::load_frame(&crate::state::state(&app), &source).map_err(err))
        .await
        .map_err(err)?
}

#[tauri::command]
pub async fn studio_save_detector(
    app: AppHandle,
    game_id: String,
    detector: DetectorDef,
    source: FrameSource,
    template_box: Option<Region>,
) -> CmdResult<GameInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        studio::save_detector(&st, &game_id, detector, &source, template_box).map_err(err)?;
        st.reload_games();
        st.game(&game_id)
            .map(|g| st.game_info(&g))
            .ok_or_else(|| "Unknown game".to_string())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn studio_test_detector(app: AppHandle, game_id: String, detector: DetectorDef, match_id: String) -> CmdResult<TestResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        let m = st
            .library
            .lock()
            .unwrap()
            .get_match(&match_id)
            .map_err(err)?
            .ok_or("Recording not found")?;
        let app2 = app.clone();
        let progress = move |p: f32| {
            let _ = app2.emit("studio-progress", serde_json::json!({ "task": "test", "progress": p }));
        };
        studio::test_detector_with_progress(&st, &game_id, detector, Path::new(&m.path), &progress).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Size, length and game picture (inside any black bars) of a recording, for Detector Studio.
#[tauri::command]
pub async fn studio_video_info(app: AppHandle, path: String) -> CmdResult<VideoInfoResult> {
    tauri::async_runtime::spawn_blocking(move || {
        studio::video_info(&crate::state::state(&app), Path::new(&path))
            .map(|(i, _)| i)
            .map_err(err)
    })
    .await
    .map_err(err)?
}

/// Stop a running Studio test early (it returns what it has so far).
#[tauri::command]
pub fn studio_cancel() {
    studio::cancel_running();
}

#[tauri::command]
pub fn studio_delete_detector(st: State<'_, AppState>, game_id: String, detector_id: String) -> CmdResult<GameInfo> {
    delete_user_detector(&st.paths.user_games, &game_id, &detector_id).map_err(err)?;
    st.reload_games();
    st.game(&game_id).map(|g| st.game_info(&g)).ok_or_else(|| "Unknown game".into())
}

#[tauri::command]
pub async fn studio_save_reader(
    app: AppHandle,
    game_id: String,
    reader: ReaderDef,
    source: Option<FrameSource>,
    crops: Vec<OptionCrop>,
) -> CmdResult<GameInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        studio::save_reader(&st, &game_id, reader, source.as_ref(), &crops).map_err(err)?;
        st.reload_games();
        st.game(&game_id)
            .map(|g| st.game_info(&g))
            .ok_or_else(|| "Unknown game".to_string())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn studio_read_frame(app: AppHandle, game_id: String, reader: ReaderDef, source: FrameSource) -> CmdResult<FrameReadResult> {
    tauri::async_runtime::spawn_blocking(move || studio::read_frame(&crate::state::state(&app), &game_id, reader, &source).map_err(err))
        .await
        .map_err(err)?
}

#[tauri::command]
pub async fn studio_test_reader(app: AppHandle, game_id: String, reader: ReaderDef, match_id: String) -> CmdResult<ReaderTestResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = crate::state::state(&app);
        let m = st
            .library
            .lock()
            .unwrap()
            .get_match(&match_id)
            .map_err(err)?
            .ok_or("Recording not found")?;
        studio::test_reader(&st, &game_id, reader, &m).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn studio_delete_reader(st: State<'_, AppState>, game_id: String, reader_id: String) -> CmdResult<GameInfo> {
    delete_user_reader(&st.paths.user_games, &game_id, &reader_id).map_err(err)?;
    st.reload_games();
    st.game(&game_id).map(|g| st.game_info(&g)).ok_or_else(|| "Unknown game".into())
}

/// Fullscreen the app window (the video player's fullscreen button).
#[tauri::command]
pub fn set_fullscreen(window: tauri::WebviewWindow, on: bool) -> CmdResult<()> {
    window.set_fullscreen(on).map_err(err)
}

#[tauri::command]
pub fn open_path(path: String) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(err)
}

/// Re-broadcast status (used by the UI on mount).
#[tauri::command]
pub fn refresh(app: AppHandle) {
    refresh_status(&app, None);
}

/// True when the app runs in end-to-end self-test mode (docs/E2E.md).
#[tauri::command]
pub fn selftest_enabled() -> bool {
    crate::selftest::enabled()
}

/// The UI reports each page of the self-test tour so the driver can screenshot it.
#[tauri::command]
pub fn selftest_tour_step(page: String) {
    crate::selftest::tour_step(&page);
}

// ---- Timeline / library previews -------------------------------------------------------------

/// Number of peaks in a recording's waveform.
const WAVE_BUCKETS: usize = 1200;
/// Frames tiled into a scrub-preview strip, and the strip's total width.
const STRIP_FRAMES: u32 = 12;
const STRIP_WIDTH: u32 = 1920;

fn recording_for(app: &AppHandle, match_id: &str) -> CmdResult<MatchRecord> {
    let st = crate::state::state(app);
    let m = st
        .library
        .lock()
        .unwrap()
        .get_match(match_id)
        .map_err(err)?
        .ok_or("Recording not found")?;
    if !Path::new(&m.path).exists() {
        return Err("The recording file is missing".into());
    }
    Ok(m)
}

/// A cache file is fresh when it exists and is no older than the recording it was made from.
fn cache_fresh(cache: &Path, video: &Path) -> bool {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(cache), modified(video)) {
        (Some(c), Some(v)) => c >= v,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Where a stream-copy clip of `start_ms..end_ms` will really start and end (the keyframe it snaps to),
/// or the exact range when frame-accurate cuts are on.
#[tauri::command]
pub async fn clip_cut_points(app: AppHandle, match_id: String, start_ms: u64, end_ms: u64) -> CmdResult<recap_core::preview::CutPoints> {
    tauri::async_runtime::spawn_blocking(move || {
        let m = recording_for(&app, &match_id)?;
        let st = crate::state::state(&app);
        let precise = st.settings.read().unwrap().auto_clip.precise_cuts;
        let end_ms = if m.duration_ms > 0 { end_ms.min(m.duration_ms) } else { end_ms };
        recap_core::preview::cut_points(&st.ff, Path::new(&m.path), start_ms, end_ms, precise).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Audio peaks (0..255) across the whole recording, evenly spaced; empty when it has no audio. Cached.
#[tauri::command]
pub async fn match_waveform(app: AppHandle, match_id: String) -> CmdResult<Vec<u8>> {
    tauri::async_runtime::spawn_blocking(move || {
        let m = recording_for(&app, &match_id)?;
        let st = crate::state::state(&app);
        let video = Path::new(&m.path);
        let cache = st.paths.thumbnails.join(format!("{}.wave", m.id));
        if cache_fresh(&cache, video) {
            if let Ok(bytes) = fs::read(&cache) {
                return Ok(bytes);
            }
        }
        let peaks = recap_core::preview::waveform(&st.ff, video, m.duration_ms, WAVE_BUCKETS).map_err(err)?;
        let _ = fs::create_dir_all(&st.paths.thumbnails);
        let _ = fs::write(&cache, &peaks);
        Ok(peaks)
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrubStrip {
    path: String,
    frames: u32,
}

/// One JPG with `frames` evenly spaced stills side by side (frame i shows the middle of slot i). Cached.
#[tauri::command]
pub async fn match_scrub_strip(app: AppHandle, match_id: String) -> CmdResult<ScrubStrip> {
    tauri::async_runtime::spawn_blocking(move || {
        let m = recording_for(&app, &match_id)?;
        let st = crate::state::state(&app);
        let video = Path::new(&m.path);
        let out = st.paths.thumbnails.join(format!("{}.strip.jpg", m.id));
        if !cache_fresh(&out, video) {
            fs::create_dir_all(&st.paths.thumbnails).map_err(err)?;
            recap_core::preview::scrub_strip(&st.ff, video, m.duration_ms, STRIP_FRAMES, STRIP_WIDTH, &out).map_err(err)?;
        }
        Ok(ScrubStrip {
            path: out.to_string_lossy().into_owned(),
            frames: STRIP_FRAMES,
        })
    })
    .await
    .map_err(err)?
}
