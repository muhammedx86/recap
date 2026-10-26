# Recapture.Events

Game event detection for [Recapture](https://github.com/muhammedx86): a separate program, `Recapture.Events.exe`,
built on Recap's event detection (this repository). It follows a game's live API (League of Legends, Counter-Strike 2,
VALORANT, Rocket League) and sends what happens, one JSON object per line, to `\\.\pipe\recapture-events`.

That pipe protocol (`events-host/src/protocol.rs`) is the whole contract with Recapture: no code is shared.

- Licence: GPL-3.0-or-later (`LICENSE`); Recap's own: `licenses/recap-LICENSE`. See `NOTICE`.
- Recap code used here and how it differs from upstream: `third_party/recap/UPSTREAM.md`.
- Build: `cargo build --release -p events-host` in this folder (gives `target/release/recapture-events.exe`).