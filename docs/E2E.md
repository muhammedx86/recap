# End-to-end test (Windows CI)

The packaged app is run for real on a Windows runner against a scripted **sample game**.
Nothing is mocked: process detection, the OBS recorder, post-match scanning, match
splitting, auto-clips, montage, Detector Studio commands and the UI all run as shipped.

## Pieces

| Piece | Where |
|---|---|
| Sample game | `crates/sample-game` → `recap-sample-game.exe` |
| Self-test mode in the app | `src-tauri` (env vars below), UI page tour in `ui/` |
| Driver script | `scripts/e2e.ps1` |
| CI job | `.github/workflows/build.yml` job `e2e` (needs the Windows build) |

## Sample game

Window 1280×720, title `Recap Sample Game`, process `recap-sample-game.exe`, software-rendered
(GDI), animated background so frames are never static. It shows high-contrast synthetic banners
at fixed times (seconds after launch), then exits at 64 s:

| t (s) | Banner (shown ~1.2–2 s) | Event kind |
|---|---|---|
| 10.0 | match start | `match_start` |
| 12.5 | round start | `round_start` |
| 15.0 | kill | `kill` |
| 18.0 | kill | `kill` |
| 20.0 | round won | `round_win` |
| 22.0 | round start | `round_start` |
| 24.0 | kill | `kill` |
| 26.5 | round lost | `round_loss` |
| 29.0 | match end | `match_end` |
| 31–45 | menu | – |
| 45.0 | match start | `match_start` |
| 47.5 | round start | `round_start` |
| 50.0 | kill | `kill` |
| 53.0 | round won | `round_win` |
| 56.0 | match end | `match_end` |
| 58.2–63.2 | end screen 2 (CANYON, 9–13 DEFEAT, you: Cross, 1 / 0 / 4) | details |
| 64.0 | exit | – |

End screen 1 is shown at 31.3–37.3 s: HARBOR, 13–11 VICTORY, three player cards with your name in
yellow (Ring, 3 / 1 / 2). Text is drawn with a system font (Segoe UI / Arial on Windows).

Absolute times in the recording depend on how fast recording starts, so checks use offsets
relative to each match's `match_start` event.

CLI:
- `recap-sample-game.exe` – run the game (Windows).
- `recap-sample-game --export-fixture <dir>` – write `<dir>/recap-sample-game/` with
  `game.json` (capture `window`, auto-clips on `kill`), `detectors/*.json` and `templates/*.png`
  drawn by the same code as the game (templateFrameHeight 720). Works on any OS.
- `recap-sample-game --export-frames <dir> --fps <n>` – render the timeline to PNG frames (any OS),
  used by a Linux test that encodes them with ffmpeg and runs the real scanner.
- `recap-sample-game --timeline` – print the table above as JSON.

## Self-test mode (environment variables)

| Variable | Effect |
|---|---|
| `RECAP_DATA_DIR` | Use this folder instead of the app data folder (settings, library, user games, thumbnails, logs). |
| `RECAP_EXTRA_GAMES` | Extra game root loaded after the bundled games (the exported fixture). |
| `RECAP_SELFTEST` | Folder for `report.json` + `tour.txt`. Enables: 5 s minimum match length for splitting; the post-processing check suite; the UI page tour. |

The driver writes `settings.json` into `RECAP_DATA_DIR` before launch (recordings folder,
30 fps, 4000 kbps, auto-clips on, split matches on).

When every sample-game match is processed, the app runs the check suite (manual clip, montage,
Studio frame load, Studio detector test, custom event type, games list) and writes
`report.json` with `"done": true`. Then the UI tours Library → Match → Clips → Studio → Games →
Settings, writing the current page name to `tour.txt` and holding each page for 4 s so the
driver can screenshot the desktop.

### report.json

```json
{
  "done": true,
  "games": [{ "id": "valorant", "name": "VALORANT", "detectors": 7, "readyDetectors": 0 }],
  "matches": [{
    "id": "…", "gameId": "recap-sample-game", "status": "ready", "durationMs": 30000,
    "path": "…", "exists": true, "sizeBytes": 123,
    "events": [{ "kind": "kill", "tMs": 15000, "source": "vision:kill" }],
    "clips": [{ "kind": "auto", "path": "…", "exists": true, "durationMs": 8000 }]
  }],
  "checks": { "manualClip": { "ok": true, "detail": "…" } },
  "recorder": {
    "armed": { "capture": "game", "encoder": "obs_x264", "buffering": true, "encoderFallback": "…" },
    "recordings": [{ "matchId": "…", "health": { "totalFrames": 1650, "droppedFrames": 0, "laggedFrames": 3, "encoder": "obs_x264", "prerollMs": 0 } }],
    "bufferLeftovers": 0
  },
  "errors": []
}
```

## What the driver asserts

- At least the 5 bundled games load, plus the sample game.
- The session was split into exactly 2 sample-game matches, both `ready`, files present.
- Match 1 (relative to its `match_start`): kills at +5, +8, +14 s; `round_start` ×2; `round_win` ×1;
  `round_loss` ×1; `match_end` at +19 s. Match 2: kill at +5 s; `round_win` ×1. Tolerance ±1.5 s.
- Each match with a kill has at least one auto-clip whose file exists.
- Match details read by the fixture's screen readers: match 1 = Ring, Harbor, 3/1/2, win, 13-11;
  match 2 = Cross, Canyon, 1/0/4, loss, 9-13 (text fields need Windows OCR; CI installs the
  en-US OCR capability if missing). A manual detail edit persists (`manualDetail` check).
- Every entry in `checks` is ok.
- The UI tour produced a screenshot for every page.
- The recorder was armed for the sample game and reported a running pre-roll buffer; every
  recording reported frame stats with frames written (dropped/lagged share is printed).
- Phase 2: the sample game plays again and the driver presses "stop" (`stop.request`) at +18 s,
  in the middle of match 1. Recording resumes in the background; the rest of match 1 is dropped
  and match 2 is recorded on its own (`report2.json`). The resumed recording starts from the
  pre-roll buffer (`prerollMs` > 0) and no recording is left in `.recap-buffer`.

Artifacts: `report.json`, screenshots, `recap.log`, `recorder.log`, the recordings.
