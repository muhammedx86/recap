# Recap

**A lightweight, local-only game recorder that finds your plays.**

Recap records your matches automatically, marks kills, deaths and other moments on a
timeline, cuts clips around them, and stitches clips into montages. Everything runs on
your PC: no account, no cloud, no subscription, no telemetry.

Plays are found by **detectors**: small data files that tell Recap where to look on
screen and what the indicator looks like. Anyone can add a game or tune a detector in
the built-in **Detector Studio** without writing code. Games with an official local
API (League of Legends, CS2) can use it as an extra, exact event source.

## Features

- **Automatic match recording** when a supported game starts, with OBS's game capture
  (window/monitor fallback) and hardware encoding (NVENC, AMF or QSV, with x264 as fallback).
- **Event timeline**: kills, deaths, assists, rounds, objectives and bookmarks, with
  click-to-seek.
- **Detectors as data files** (`games/<game>/detectors/*.json` plus reference images),
  with per-detector region, threshold, cooldown and sample rate. See [docs/DETECTORS.md](docs/DETECTORS.md).
- **Detector Studio**: open a recording or screenshot, drag a box around an indicator,
  test it against a whole match with a live score graph, then save.
- **Optional official local APIs**: League of Legends / TFT Live Client Data API and
  CS2 Game State Integration.
- **Post-match processors** for files the game writes itself (replays, demos), as
  pluggable commands.
- **Bookmark hotkey** (F8 by default) that works in every game, as a fallback for
  anything detection misses. Manual recording hotkey (F9).
- **Auto-clips** around chosen events (5 s before, 3 s after by default, overlapping
  clips merged), **manual clips** from the player, and **montages**.
- Instant stream-copy cuts, with an optional frame-accurate re-encode.

Bundled games: League of Legends / TFT, VALORANT, Counter-Strike 2, Rocket League, Marvel Rivals,
Rainbow Six Siege, Apex Legends. Rocket League plays (goals, assists, saves, epic saves, shots,
demos, goals against, MVP) come from the game's own Stats API, which Recap switches on in
`TAGame\Config\DefaultStatsAPI.ini` (restart the game once afterwards).
Vision detectors ship with regions but **no reference images**. You capture those
from your own recordings in Detector Studio (a minute per detector), so they match
your resolution, HUD scale and settings.

## Privacy

Recap makes **no network connections** of its own. It talks only to:

- the games' own local APIs on `127.0.0.1` (League Live Client Data, CS2 GSI), when
  enabled for that game;
- the recorder process it starts on your PC.

Recordings, clips, events and settings stay in folders you choose. There are no
accounts, uploads, analytics or crash reporting.

## How it works

```text
┌────────────────────────── Recap (Tauri app) ──────────────────────────┐
│  React UI  ── library · player/timeline · clips · montage · studio    │
│     │ invoke / events                                                 │
│  Rust backend                                                         │
│   ├─ process watcher ── game.json `processes` ──► start / stop match  │
│   ├─ recorder client ── JSON lines over stdio ──┐                     │
│   ├─ API sources (optional, live)               │                     │
│   │    League Live Client Data · CS2 GSI ──┐    │                     │
│   ├─ bookmark hotkey ──────────────────────┤    │                     │
│   ├─ post-match:                           ▼    │                     │
│   │    vision scan (detectors) ──► GameEvent ──► SQLite library       │
│   │    post-match processors ───┘        │                            │
│   └─ clips / montage (ffmpeg, stream copy) ◄─┘                        │
└───────────────────────────────────────────────────────────────────────┘
                     │ stdin/stdout
             ┌───────▼────────┐
             │ recap-recorder │  libobs (OBS engine): game/window/monitor
             │  (sidecar)     │  capture, desktop + mic audio, NVENC/AMF/QSV
             └────────────────┘
```

| Path | What |
|---|---|
| `crates/recap-core` | Platform-neutral core: event model, detector files, ZNCC matcher, video scan, clip planning, League/CS2 parsers, SQLite library. Fully unit-tested on any OS. |
| `crates/recap-recorder` | Recorder sidecar built on [libobs-rs](https://github.com/libobs-rs/libobs-rs). Runs in its own process so an OBS crash can't take the app down. |
| `src-tauri` | Tauri v2 app backend: process watcher, sessions, hotkeys, API sources, commands. |
| `ui` | React + TypeScript UI. |
| `games` | Bundled game and detector data files. |
| `docs` | [App contract](docs/API.md), [detector guide](docs/DETECTORS.md). |

## Install

Download the latest installer (or the portable zip) from
[Releases](https://github.com/ywillxia/recap/releases), or the `recap-installer`
artifact from the newest successful [build](https://github.com/ywillxia/recap/actions) run.
Windows 10/11, 64-bit. Some games need Recap to run as administrator for game capture.

## Build from source

Requirements: Windows 10/11, [Rust](https://rustup.rs) (stable, MSVC), Node.js 22,
and the WebView2 runtime (preinstalled on Windows 11).

```powershell
# 1. Tools
cargo install cargo-obs-build --version 2.0.3 --locked

# 2. UI dependencies
npm --prefix ui install

# 3. Build the recorder and put it next to the OBS runtime (same layout as CI)
cargo build -p recap-recorder
cargo obs-build build --out-dir src-tauri/obs       # run from the repo root
copy target\debug\recap-recorder.exe src-tauri\obs\

# 4. ffmpeg + ffprobe: put them on PATH (or in src-tauri/binaries/)
#    e.g. https://github.com/BtbN/FFmpeg-Builds/releases (win64-gpl)

# 5. Run the app in dev mode
npx @tauri-apps/cli@2 dev
```

A release build with installer is produced by `.github/workflows/build.yml`, which
places the OBS runtime and `recap-recorder.exe` in `src-tauri/obs/`, ffmpeg in
`src-tauri/binaries/`, and runs `npx @tauri-apps/cli@2 build`.

Core tests run anywhere:

```bash
cargo test -p recap-core
```

## Adding a game

Create `%APPDATA%\com.recap.app\games\<id>\game.json` (or use **Games → Add game**
in the app), then build detectors for it in Detector Studio. Full reference:
[docs/DETECTORS.md](docs/DETECTORS.md). Pull requests adding detectors for more
games are welcome.

## License

Recap is free software under the **GNU General Public License v3.0 or later**
([LICENSE](LICENSE)).

Recap builds on the OBS Studio engine (libobs, GPL-2.0-or-later) through
[libobs-rs](https://github.com/libobs-rs/libobs-rs) (GPL-3.0), and bundles an ffmpeg
GPL build. Because of this, any distributed derivative of Recap must also be GPL.

Recap is not affiliated with or endorsed by Riot Games, Valve, NetEase, Marvel, or
the OBS Project. Game names are trademarks of their owners.
