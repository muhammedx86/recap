//! After a match: thumbnail → vision scan → post-match processors → auto-clips.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use recap_core::{
    clips::{cut_clip, kinds_in_window, plan_auto_clips},
    events::{kind, merge_events, EventSource, GameEvent},
    games::{ApiSource, LoadedDetector, LoadedGame},
    library::{ClipKind, ClipRecord, MatchRecord, MatchStatus},
    postmatch::{run_processor, PostMatchContext},
    scan::{scan_video, ScanOptions},
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{
    paths::sanitize,
    state::{now_unix_ms, state, AppState},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    match_id: &'a str,
    stage: &'a str,
    progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit_progress(app: &AppHandle, match_id: &str, stage: &str, progress: f32, message: Option<String>) {
    let _ = app.emit(
        "processing-progress",
        Progress {
            match_id,
            stage,
            progress,
            message,
        },
    );
}

/// Event kinds an API source reports exactly; vision detectors for these are skipped when `preferApi` is set.
pub fn api_kinds(api: ApiSource) -> &'static [&'static str] {
    match api {
        ApiSource::LeagueLiveClient => &[kind::KILL, kind::DEATH, kind::ASSIST, kind::MULTIKILL, kind::ACE, kind::OBJECTIVE],
        ApiSource::Cs2Gsi => &[
            kind::KILL,
            kind::DEATH,
            kind::ASSIST,
            kind::MVP,
            kind::ROUND_WIN,
            kind::ROUND_LOSS,
            kind::BOMB_PLANT,
            kind::BOMB_DEFUSE,
        ],
    }
}

pub fn detectors_to_run(game: &LoadedGame) -> Vec<LoadedDetector> {
    let skip: &[&str] = match (game.def.api, game.def.prefer_api) {
        (Some(api), true) => api_kinds(api),
        _ => &[],
    };
    game.ready_detectors()
        .filter(|d| !skip.contains(&d.def.event.as_str()))
        .cloned()
        .collect()
}

/// Process a match on a background thread. `rescan` clears previous vision/post-match events and auto-clips first.
pub fn spawn(app: AppHandle, match_id: String, rescan: bool) {
    {
        let st = state(&app);
        let mut p = st.processing.lock().unwrap();
        if !p.insert(match_id.clone()) {
            log::info!("{match_id} is already processing");
            return;
        }
    }
    std::thread::Builder::new()
        .name("processing".into())
        .spawn(move || {
            let result = run(&app, &match_id, rescan);
            let st = state(&app);
            st.processing.lock().unwrap().remove(&match_id);
            let lib = st.library.lock().unwrap();
            if let Ok(Some(mut m)) = lib.get_match(&match_id) {
                match &result {
                    Ok(()) => {
                        m.status = MatchStatus::Ready;
                        m.error = None;
                    }
                    Err(e) => {
                        m.status = MatchStatus::Error;
                        m.error = Some(format!("{e:#}"));
                    }
                }
                let _ = lib.update_match(&m);
            }
            drop(lib);
            match result {
                Ok(()) => emit_progress(&app, &match_id, "done", 1.0, None),
                Err(e) => {
                    log::error!("processing {match_id}: {e:#}");
                    emit_progress(&app, &match_id, "error", 1.0, Some(format!("{e:#}")));
                }
            }
            let _ = app.emit("library-changed", serde_json::json!({ "matchId": match_id }));
            enforce_storage_limit(&app);
        })
        .ok();
}

fn run(app: &AppHandle, match_id: &str, rescan: bool) -> Result<()> {
    let st = state(app);
    let mut m = st.library.lock().unwrap().get_match(match_id)?.context("match not found")?;
    let video = PathBuf::from(&m.path);
    anyhow::ensure!(video.exists(), "recording file is missing: {}", video.display());

    {
        let lib = st.library.lock().unwrap();
        m.status = MatchStatus::Processing;
        lib.update_match(&m)?;
    }

    // Duration and size from the file itself.
    if let Ok(info) = st.ff.probe(&video) {
        m.duration_ms = info.duration_ms;
    }
    m.size_bytes = fs::metadata(&video).map(|x| x.len()).unwrap_or(m.size_bytes);

    // Thumbnail.
    fs::create_dir_all(&st.paths.thumbnails)?;
    let thumb = st.paths.thumbnails.join(format!("{match_id}.jpg"));
    let at = (m.duration_ms / 3).min(20_000);
    if st.ff.thumbnail(&video, at, &thumb, 480).is_ok() {
        m.thumbnail = Some(thumb.to_string_lossy().into_owned());
    }
    st.library.lock().unwrap().update_match(&m)?;

    if rescan {
        let lib = st.library.lock().unwrap();
        lib.clear_derived_events(match_id)?;
        for c in lib.list_clips(Some(match_id))?.into_iter().filter(|c| c.kind == ClipKind::Auto) {
            let _ = fs::remove_file(&c.path);
            if let Some(t) = &c.thumbnail {
                let _ = fs::remove_file(t);
            }
            lib.delete_clip(&c.id)?;
        }
    }

    let game = st.game(&m.game_id);
    let settings = st.settings.read().unwrap().clone();
    let mut derived: Vec<GameEvent> = Vec::new();

    // Vision scan.
    if let Some(g) = &game {
        let dets = detectors_to_run(g);
        if settings.scan_after_match && !dets.is_empty() {
            emit_progress(app, match_id, "scanning", 0.0, None);
            let app2 = app.clone();
            let mid = match_id.to_string();
            let progress = move |p: f32| emit_progress(&app2, &mid, "scanning", p, None);
            let res = scan_video(
                &st.ff,
                &video,
                &dets,
                ScanOptions {
                    analysis_height: settings.analysis_height,
                    keep_traces: false,
                    cancel: None,
                    progress: Some(&progress),
                },
            )?;
            derived.extend(res.events);
        }

        // Post-match processors.
        if !g.def.post_match.is_empty() {
            emit_progress(app, match_id, "postMatch", 0.0, None);
            let ctx = PostMatchContext {
                recording: video.clone(),
                match_start_unix_ms: m.started_unix_ms,
                match_end_unix_ms: m.started_unix_ms + m.duration_ms,
                game_dir: g.dir.clone(),
            };
            for def in &g.def.post_match {
                match run_processor(def, &ctx, Duration::from_secs(300)) {
                    Ok(ev) => derived.extend(ev),
                    Err(e) => log::warn!("post-match processor failed: {e:#}"),
                }
            }
        }
    }

    // Merge with live events (API + manual) and store only the new ones.
    let existing = st.library.lock().unwrap().list_events(match_id)?;
    let existing_len = existing.len();
    let merged = merge_events(existing.into_iter().chain(derived).collect(), 1500);
    let new: Vec<GameEvent> = merged
        .into_iter()
        .filter(|e| e.id.is_none() && matches!(e.source, EventSource::Vision { .. } | EventSource::PostMatch { .. }))
        .collect();
    if !new.is_empty() {
        st.library.lock().unwrap().add_events(match_id, &new)?;
    }
    log::info!("{match_id}: {} live events, {} derived events added", existing_len, new.len());

    // Auto-clips.
    if settings.auto_clip.enabled {
        let events = st.library.lock().unwrap().list_events(match_id)?;
        let kinds: Vec<String> = if !settings.auto_clip.events_override.is_empty() {
            settings.auto_clip.events_override.clone()
        } else if let Some(g) = &game {
            g.def.auto_clip.events.clone()
        } else {
            vec![kind::BOOKMARK.to_string()]
        };
        let mut kinds = kinds;
        if !kinds.iter().any(|k| k == kind::BOOKMARK) {
            // Bookmarks are an explicit "clip this", always honoured.
            kinds.push(kind::BOOKMARK.into());
        }
        let windows = plan_auto_clips(
            &events,
            &kinds,
            (settings.auto_clip.seconds_before * 1000.0) as u64,
            (settings.auto_clip.seconds_after * 1000.0) as u64,
            (settings.auto_clip.merge_gap_seconds * 1000.0) as u64,
            m.duration_ms,
        );
        let total = windows.len().max(1) as f32;
        for (i, w) in windows.iter().enumerate() {
            emit_progress(app, match_id, "clipping", i as f32 / total, None);
            let title = clip_title(&w.event_kinds, w.anchor_ms);
            match make_clip(&st, &m, w.start_ms, w.end_ms, &title, ClipKind::Auto, &events) {
                Ok(_) => {}
                Err(e) => log::warn!("auto-clip failed: {e:#}"),
            }
        }
    }
    Ok(())
}

fn clip_title(kinds: &[String], anchor_ms: u64) -> String {
    let main = kinds
        .iter()
        .find(|k| k.as_str() != kind::MATCH_END)
        .cloned()
        .unwrap_or_else(|| "clip".into());
    let label = main.replace('_', " ");
    let mut c = label.chars();
    let label = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default();
    format!("{label} @ {}:{:02}", anchor_ms / 60_000, (anchor_ms / 1000) % 60)
}

pub fn clips_dir(st: &AppState, game_name: &str) -> PathBuf {
    st.recordings_dir().join("Clips").join(sanitize(game_name))
}

/// Cut a clip from a match and record it in the library.
pub fn make_clip(
    st: &AppState,
    m: &MatchRecord,
    start_ms: u64,
    end_ms: u64,
    title: &str,
    kind: ClipKind,
    events: &[GameEvent],
) -> Result<ClipRecord> {
    anyhow::ensure!(end_ms > start_ms + 200, "clip is too short");
    let dir = clips_dir(st, &m.game_name);
    fs::create_dir_all(&dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let out = unique_path(&dir, &format!("{stamp}_{}", sanitize(title)), "mp4");
    let precise = st.settings.read().unwrap().auto_clip.precise_cuts;
    cut_clip(&st.ff, Path::new(&m.path), start_ms, end_ms, &out, precise)?;
    let thumb = st.paths.thumbnails.join(format!("clip-{id}.jpg"));
    let thumbnail = st
        .ff
        .thumbnail(&out, ((end_ms - start_ms) / 2).min(3000), &thumb, 360)
        .ok()
        .map(|_| thumb.to_string_lossy().into_owned());
    let duration_ms = st.ff.probe(&out).map(|i| i.duration_ms).unwrap_or(end_ms - start_ms);
    let clip = ClipRecord {
        id,
        match_id: Some(m.id.clone()),
        path: out.to_string_lossy().into_owned(),
        start_ms,
        end_ms,
        duration_ms,
        kind,
        title: title.to_string(),
        event_kinds: kinds_in_window(events, start_ms, end_ms),
        created_unix_ms: now_unix_ms(),
        thumbnail,
        size_bytes: fs::metadata(&out).map(|x| x.len()).unwrap_or(0),
    };
    st.library.lock().unwrap().insert_clip(&clip)?;
    Ok(clip)
}

pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    p
}

/// Delete the oldest non-favourite recordings until under the storage limit.
pub fn enforce_storage_limit(app: &AppHandle) {
    let st = state(app);
    let Some(limit_gb) = st.settings.read().unwrap().storage_limit_gb else {
        return;
    };
    let limit = (limit_gb as f64 * 1024.0 * 1024.0 * 1024.0) as u64;
    // Lock order: never hold the library lock while taking session/processing.
    let active = st.session.lock().unwrap().as_ref().map(|s| s.match_id.clone());
    let busy = st.processing.lock().unwrap().clone();
    let (mut used, candidates) = {
        let lib = st.library.lock().unwrap();
        let Ok(all) = lib.list_matches(None) else { return };
        let used: u64 = all.iter().map(|m| m.size_bytes).sum();
        if used <= limit {
            return;
        }
        let Ok(c) = lib.oldest_deletable_matches() else { return };
        (used, c)
    };
    let mut deleted = false;
    for m in candidates {
        if used <= limit {
            break;
        }
        if Some(&m.id) == active.as_ref() || busy.contains(&m.id) {
            continue;
        }
        log::info!("storage limit: deleting {} ({} bytes)", m.path, m.size_bytes);
        if st.library.lock().unwrap().delete_match(&m.id).is_ok() {
            let _ = fs::remove_file(&m.path);
            if let Some(t) = &m.thumbnail {
                let _ = fs::remove_file(t);
            }
            used = used.saturating_sub(m.size_bytes);
            deleted = true;
        }
    }
    if deleted {
        let _ = app.emit("library-changed", serde_json::json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::clip_title;

    #[test]
    fn titles() {
        assert_eq!(clip_title(&["round_win".into()], 125_000), "Round win @ 2:05");
        assert_eq!(clip_title(&[], 0), "Clip @ 0:00");
    }
}
