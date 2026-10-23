# recap-recorder

Recap's recording engine: a small Windows program that hosts **libobs** (OBS Studio's
engine, via the [libobs-rs](https://github.com/libobs-rs/libobs-rs) crates) and is driven
by the Recap app over stdin/stdout. Running OBS in its own process means a crash in a
capture hook or encoder can never take the app down, and the app always gets a
finalized file (the output uses OBS's hybrid MP4 muxer, which stays playable even if the
process dies).

## Protocol

JSON lines, defined in [`recap_core::protocol`](../recap-core/src/protocol.rs):

- **stdin** (app → recorder): `start`, `stop`, `listEncoders`, `shutdown`.
- **stdout** (recorder → app): `ready`, `started`, `captureChanged`, `stopped`, `encoders`, `error`.
- **stderr**: human-readable logs, including OBS's own log (the app should capture it into its log file).

```text
<- {"event":"ready","obsVersion":"32.0.4"}
-> {"cmd":"start","id":"m1","path":"C:\\Videos\\Recap\\m1.mp4","capture":{"mode":"game","process":"VALORANT-Win64-Shipping.exe"},"video":{"fps":60,"bitrateKbps":12000,"encoder":"auto"},"audio":{"desktop":true,"microphone":false}}
<- {"event":"started","id":"m1","path":"…","startedUnixMs":1792800000000,"encoder":"obs_nvenc_h264_tex","capture":"game"}
-> {"cmd":"stop","id":"m1"}
<- {"event":"stopped","id":"m1","path":"…","durationMs":1803455}
```

Behaviour:

- **One recording at a time.** A second `start` gets an `error`.
- **Game / window capture** looks for the game's window by pid or exe name (case-insensitive,
  game windows preferred) and keeps looking for up to 60 s while the game boots. If no window
  appears it records the monitor instead and sends `captureChanged` before `started`.
  Game capture uses OBS's anti-cheat compatibility hook; the cursor is not captured.
- **Stop while still waiting** for the window answers with `stopped` (`durationMs: 0`) and no file.
- **Encoders:** `auto` prefers NVENC, then AMF, then QuickSync, then x264. `nvenc`/`amf`/`qsv`
  fall back to `auto` when that family is not available. If a hardware encoder fails to start,
  the recorder retries once with x264. Keyframes are every 1 s so the app's stream-copy clip cuts
  land close to the requested time.
- **Canvas** is the primary monitor's resolution; `outputHeight` scales the output down (never up).
- **Audio:** desktop audio (default output device) and optionally a microphone, mixed into one AAC track.
- **stdin closing** means the app died: the recorder stops and finalizes any recording, then exits.

## OBS binaries

libobs is loaded at runtime from DLLs that must sit next to `recap-recorder.exe`, with the
`data/` and `obs-plugins/` folders OBS expects. The recorder resolves those paths relative to its
own executable and also sets its working directory there, so the app can spawn it from anywhere.

The binaries are fetched with [`cargo-obs-build`](https://crates.io/crates/cargo-obs-build). It
reads `libobs-version` from this crate's `[package.metadata]`, so **run it from this folder**:

```powershell
cargo install cargo-obs-build

# Development: put OBS next to the debug binary
cd crates/recap-recorder
cargo obs-build build --out-dir ../../target/debug
cd ../..
cargo build -p recap-recorder
```

`libobs-version` must match the `libobs` bindings crate that `libobs-wrapper` resolves to in
`Cargo.lock` (currently `libobs 5.0.1+32.0.4`, i.e. OBS 32.0.4). Bump both together.

### Release layout

CI builds `recap-recorder.exe` in release mode, runs `cargo obs-build build --out-dir src-tauri/obs`
(from this folder, with the path adjusted), and copies `recap-recorder.exe` into `src-tauri/obs/`.
The app bundles that folder as the resource directory `obs/` and spawns
`<resources>/obs/recap-recorder.exe`.

```text
obs/
  recap-recorder.exe
  obs.dll, libobs-d3d11.dll, avcodec-*.dll, …
  data/libobs/…, data/obs-plugins/…
  obs-plugins/64bit/win-capture.dll, obs-ffmpeg.dll, obs-nvenc.dll, …
```

Game capture needs OBS's `inject-helper` / `graphics-hook` files from `win-capture`, which
`cargo obs-build` includes.

## Development notes

- The real backend is Windows-only (`src/obs_backend.rs`). On other platforms the binary just
  reports `recap-recorder only supports Windows`.
- `cargo test -p recap-recorder` runs the protocol/session tests and the encoder and window
  selection tests on any platform.
- To type-check the OBS backend on Linux (everything except the Windows capture sources), install
  libclang and the [SIMDe](https://github.com/simd-everywhere/simde) headers, then:

  ```bash
  BINDGEN_EXTRA_CLANG_ARGS="-I/path/to/simde" LIBOBS_PATH=/tmp/empty \
    cargo check -p recap-recorder --features linux-typecheck
  ```

## License

GPL-3.0-or-later, like the rest of Recap (libobs and libobs-rs are GPL).
