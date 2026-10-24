//! End-to-end self-test mode (see docs/E2E.md). Off unless `RECAP_SELFTEST` is set.
//!
//! After the sample game's session has been recorded and processed, run the remaining
//! features against the real recordings (manual clip, montage, Studio frame + detector test,
//! custom event type, encoder list), write `report.json`, then ask the UI to tour every page
//! while the driver script takes screenshots.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use recap_core::{
    events::EventTypeDef,
    games::upsert_event_type,
    library::{ClipKind, MatchStatus},
};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::{
    processing,
    state::{state, AppState},
    studio::{self, FrameSource},
};

pub const SAMPLE_GAME: &str = "recap-sample-game";

static STARTED: AtomicBool = AtomicBool::new(false);
/// Phase 2: the driver asked for a manual stop and the resumed recording finished.
static PHASE2_STOPS: AtomicBool = AtomicBool::new(false);

/// The driver writes `stop.request` to press "stop" like the player would (phase 2).
pub fn poll_stop_request(app: &AppHandle) {
    let Some(d) = dir() else { return };
    let req = d.join("stop.request");
    if req.exists() {
        let _ = fs::remove_file(&req);
        log::info!("self-test: manual stop requested");
        crate::monitor::on_manual_stop(app);
        let _ = crate::session::stop(app);
    }
    // Phase 2 is done recording once its game has exited and nothing is recording.
    if d.join("phase2.txt").exists()
        && d.join("phase2-game-exited").exists()
        && state(app).session.lock().unwrap().is_none()
        && !PHASE2_STOPS.swap(true, Ordering::SeqCst)
    {
        // Processing may already have finished: check now too.
        after_processing(app);
    }
}

/// Folder for report.json / tour.txt when self-test mode is on.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("RECAP_SELFTEST").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn enabled() -> bool {
    dir().is_some()
}

/// Matches shorter than this are not split out (the sample game's matches are ~20 s).
pub fn min_match_len_ms() -> u64 {
    if enabled() {
        5_000
    } else {
        recap_core::segments::SegmentOptions::default().min_len_ms
    }
}

/// Called after every processing run. Starts the check suite once the sample game's
/// session is fully processed.
pub fn after_processing(app: &AppHandle) {
    let Some(out) = dir() else { return };
    let st = state(app);
    if st.session.lock().unwrap().is_some() || !st.processing.lock().unwrap().is_empty() {
        return;
    }
    // Phase 2 (manual stop mid-match): once both of its recordings are processed, list the matches.
    if STARTED.load(Ordering::SeqCst) && out.join("phase2.txt").exists() {
        let ids_before: Vec<String> = fs::read_to_string(out.join("phase2.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let lib = st.library.lock().unwrap();
        let all = lib.list_matches(Some(SAMPLE_GAME)).unwrap_or_default();
        let new: Vec<_> = all.iter().filter(|m| !ids_before.contains(&m.id)).collect();
        let settled = new.iter().all(|m| matches!(m.status, MatchStatus::Ready | MatchStatus::Error));
        // The stopped recording and the resumed one (both processed).
        if settled && PHASE2_STOPS.load(Ordering::SeqCst) && !new.is_empty() && !out.join("report2.json").exists() {
            let mut v: Vec<Value> = new
                .iter()
                .map(|m| {
                    let events: Vec<Value> = lib
                        .list_events(&m.id)
                        .unwrap_or_default()
                        .iter()
                        .filter(|e| !e.kind.starts_with('_'))
                        .map(|e| json!({ "kind": e.kind, "tMs": e.t_ms }))
                        .collect();
                    json!({ "id": m.id, "startedUnixMs": m.started_unix_ms, "durationMs": m.duration_ms, "status": m.status, "events": events })
                })
                .collect();
            v.sort_by_key(|m| m["startedUnixMs"].as_u64().unwrap_or(0));
            write_json(&out.join("report2.json"), &json!({ "matches": v }));
            log::info!("self-test phase 2 report written");
        }
        return;
    }
    let matches = st.library.lock().unwrap().list_matches(Some(SAMPLE_GAME)).unwrap_or_default();
    let finished = !matches.is_empty() && matches.iter().all(|m| matches!(m.status, MatchStatus::Ready | MatchStatus::Error));
    if !finished || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let st = state(&app);
        let checks = run_checks(&st);
        let report = build_report(&st, checks);
        write_json(&out.join("report.json"), &report);
        log::info!("self-test report written to {}", out.display());
        let first = report["matches"]
            .as_array()
            .and_then(|m| m.first())
            .and_then(|m| m["id"].as_str())
            .unwrap_or("")
            .to_string();
        let _ = app.emit("library-changed", json!({}));
        let _ = app.emit("selftest-tour", json!({ "matchId": first }));
    });
}

/// The UI reports which page it is showing during the tour.
pub fn tour_step(page: &str) {
    if let Some(d) = dir() {
        let _ = fs::write(d.join("tour.txt"), page);
    }
}

fn check(name: &str, f: impl FnOnce() -> anyhow::Result<Value>) -> (String, Value) {
    let v = match f() {
        Ok(detail) => json!({ "ok": true, "detail": detail }),
        Err(e) => json!({ "ok": false, "detail": format!("{e:#}") }),
    };
    log::info!("self-test {name}: {v}");
    (name.to_string(), v)
}

fn run_checks(st: &AppState) -> Value {
    let mut out = serde_json::Map::new();
    let matches = st.library.lock().unwrap().list_matches(Some(SAMPLE_GAME)).unwrap_or_default();
    let mut ok_matches: Vec<_> = matches.into_iter().filter(|m| m.status == MatchStatus::Ready).collect();
    ok_matches.sort_by_key(|m| m.started_unix_ms);

    let (k, v) = check("bundledGames", || {
        let games = st.games.read().unwrap();
        let ids: Vec<&str> = games.iter().map(|g| g.def.id.as_str()).collect();
        for want in ["valorant", "cs2", "league-of-legends", "marvel-rivals", "r6-siege", SAMPLE_GAME] {
            anyhow::ensure!(ids.contains(&want), "missing game {want}; loaded {ids:?}");
        }
        Ok(json!(ids))
    });
    out.insert(k, v);

    let (k, v) = check("encoders", || {
        let list = st.recorder.list_encoders()?;
        anyhow::ensure!(!list.is_empty(), "no encoders reported");
        Ok(json!(list))
    });
    out.insert(k, v);

    let first = ok_matches.first().cloned();
    let (k, v) = check("manualClip", || {
        let m = first.clone().ok_or_else(|| anyhow::anyhow!("no processed sample match"))?;
        let events = st.library.lock().unwrap().list_events(&m.id)?;
        let end = m.duration_ms.clamp(3_000, 9_000);
        let clip = processing::make_clip(st, &m, 2_000, end, "Self-test clip", ClipKind::Manual, &events)?;
        anyhow::ensure!(Path::new(&clip.path).exists(), "clip file missing");
        Ok(json!({ "path": clip.path, "durationMs": clip.duration_ms }))
    });
    out.insert(k, v);

    let (k, v) = check("montage", || {
        let clips = st.library.lock().unwrap().list_clips(None)?;
        let ids: Vec<String> = clips
            .iter()
            .filter(|c| c.kind != ClipKind::Montage)
            .take(2)
            .map(|c| c.id.clone())
            .collect();
        anyhow::ensure!(ids.len() == 2, "need two clips, have {}", ids.len());
        let m = processing::make_montage(st, &ids, "Self-test montage")?;
        anyhow::ensure!(Path::new(&m.path).exists() && m.duration_ms > 0, "montage file missing or empty");
        Ok(json!({ "path": m.path, "durationMs": m.duration_ms }))
    });
    out.insert(k, v);

    let (k, v) = check("studioFrame", || {
        let m = first.clone().ok_or_else(|| anyhow::anyhow!("no processed sample match"))?;
        let f = studio::load_frame(
            st,
            &FrameSource::Video {
                path: m.path.clone(),
                t_ms: 1_000,
            },
        )?;
        anyhow::ensure!(f.width > 0 && f.height > 0 && f.data_url.starts_with("data:image/png"), "bad frame");
        Ok(json!({ "width": f.width, "height": f.height }))
    });
    out.insert(k, v);

    let (k, v) = check("studioTest", || {
        let m = first.clone().ok_or_else(|| anyhow::anyhow!("no processed sample match"))?;
        let game = st.game(SAMPLE_GAME).ok_or_else(|| anyhow::anyhow!("sample game not loaded"))?;
        let det = game
            .detectors
            .iter()
            .find(|d| d.def.event == "kill")
            .ok_or_else(|| anyhow::anyhow!("no kill detector"))?;
        let res = studio::test_detector(st, SAMPLE_GAME, det.def.clone(), Path::new(&m.path))?;
        anyhow::ensure!(
            !res.events.is_empty(),
            "kill detector found nothing ({} samples)",
            res.samples.len()
        );
        Ok(json!({ "hits": res.events.iter().map(|e| e.t_ms).collect::<Vec<_>>(), "samples": res.samples.len() }))
    });
    out.insert(k, v);

    let (k, v) = check("manualDetail", || {
        let m = first.clone().ok_or_else(|| anyhow::anyhow!("no processed sample match"))?;
        let lib = st.library.lock().unwrap();
        let mut rec = lib.get_match(&m.id)?.ok_or_else(|| anyhow::anyhow!("match vanished"))?;
        recap_core::details::offer(
            &mut rec.details,
            "mode",
            recap_core::details::DetailValue::new("Self-test", recap_core::details::DetailSource::Manual),
        );
        lib.update_match(&rec)?;
        let back = lib.get_match(&m.id)?.ok_or_else(|| anyhow::anyhow!("match vanished"))?;
        let v = back.details.get("mode").ok_or_else(|| anyhow::anyhow!("manual detail not saved"))?;
        anyhow::ensure!(
            v.value == "Self-test" && v.source == recap_core::details::DetailSource::Manual,
            "saved as {v:?}"
        );
        Ok(json!(v))
    });
    out.insert(k, v);

    let (k, v) = check("eventType", || {
        let game = st.game(SAMPLE_GAME).ok_or_else(|| anyhow::anyhow!("sample game not loaded"))?;
        let t = EventTypeDef {
            id: String::new(),
            label: "Self-test marker".into(),
            icon: "zap".into(),
            color: String::new(),
            builtin: false,
        };
        upsert_event_type(&st.paths.user_games, &game.def, t)?;
        st.reload_games();
        let g = st
            .game(SAMPLE_GAME)
            .ok_or_else(|| anyhow::anyhow!("sample game vanished after reload"))?;
        anyhow::ensure!(g.event_types().iter().any(|t| t.id == "self_test_marker"), "custom type not loaded");
        Ok(json!("self_test_marker"))
    });
    out.insert(k, v);

    Value::Object(out)
}

fn build_report(st: &AppState, checks: Value) -> Value {
    let lib = st.library.lock().unwrap();
    let games: Vec<Value> = st
        .games
        .read()
        .unwrap()
        .iter()
        .map(|g| json!({ "id": g.def.id, "name": g.def.name, "detectors": g.detectors.len(), "readyDetectors": g.ready_detectors().count() }))
        .collect();
    let mut matches: Vec<Value> = Vec::new();
    let mut list = lib.list_matches(None).unwrap_or_default();
    list.sort_by_key(|m| m.started_unix_ms);
    for m in list {
        let events: Vec<Value> = lib
            .list_events(&m.id)
            .unwrap_or_default()
            .iter()
            .map(|e| json!({ "kind": e.kind, "tMs": e.t_ms, "source": e.source.label(), "confidence": e.confidence }))
            .collect();
        let clips: Vec<Value> = lib
            .list_clips(Some(&m.id))
            .unwrap_or_default()
            .iter()
            .map(|c| json!({ "kind": c.kind, "path": c.path, "exists": Path::new(&c.path).exists(), "durationMs": c.duration_ms }))
            .collect();
        matches.push(json!({
            "id": m.id, "gameId": m.game_id, "status": m.status, "error": m.error, "durationMs": m.duration_ms,
            "path": m.path, "exists": Path::new(&m.path).exists(), "sizeBytes": m.size_bytes,
            "events": events, "clips": clips, "details": m.details,
        }));
    }
    let montages: Vec<Value> = lib
        .list_clips(None)
        .unwrap_or_default()
        .iter()
        .filter(|c| c.kind == ClipKind::Montage)
        .map(|c| json!({ "path": c.path, "exists": Path::new(&c.path).exists(), "durationMs": c.duration_ms }))
        .collect();
    json!({ "done": true, "games": games, "matches": matches, "montages": montages, "checks": checks, "errors": [], "ocrAvailable": recap_ocr::available() })
}

fn write_json(path: &Path, v: &Value) {
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}
