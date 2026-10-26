# Recap (vendored)

- Upstream: https://github.com/ywillxia/recap
- Commit: 11df5127932bd9cb87f1b31494d2bc04946766cb (2026-10-25)
- Licence: GPL-3.0-or-later (../../licenses/recap-LICENSE)
- Copied: `crates/recap-core` → `recap-core/`, `games/` → `games/`, `docs/DETECTORS.md`.

## What Recapture.Events uses

events, games, vision, live, scan, ffmpeg, clips, matchgate, postmatch, details, readers,
segments, cs2, league, riot_local, rocket_league, valorant_log.

## Changes from upstream

1. Removed Recap app glue not used here: `library` (SQLite), `prebuffer`, `preview`,
   `protocol` (OBS recorder sidecar), `settings`, `background`; dropped the `rusqlite`
   and `uuid` dependencies with them.
2. `tests/bundled_games.rs`: games folder path is `../games` (layout above).
3. `events-host/src/sources.rs` is adapted from upstream `src-tauri/src/sources.rs` (League poller,
   CS2 GSI listener + config install, Rocket League stream reader) and its Riot local polling,
   feeding a channel instead of Recap's session state.
