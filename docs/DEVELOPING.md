# Running Recap from source (Windows)

Fast iteration without installers. Everything happens in your clone of the repository.

## One-time setup (about 10 minutes)

Prerequisites (each is one command; the setup script tells you if one is missing):

```powershell
winget install Rustlang.Rustup
winget install OpenJS.NodeJS.LTS
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then, from the repository root:

```powershell
pwsh scripts/dev-setup.ps1
```

It installs the UI packages, downloads the OBS runtime (`src-tauri/obs`) and ffmpeg
(`src-tauri/binaries`), and builds the recorder. None of these files are committed.

## Every day

| Command | What you get | Turnaround |
|---|---|---|
| `pwsh scripts/dev.ps1 -UiOnly` | The UI in a browser at http://localhost:1420 with demo data | UI edits appear instantly |
| `pwsh scripts/dev.ps1` | The real app (debug build), recording games for real | UI edits instantly; Rust edits rebuild and restart the app (seconds to a minute) |
| `pwsh scripts/dev.ps1 -Release` | Optimised build, closest to the installer | Slower builds |

The dev app is called **Recap (dev)** and has its own settings, library and Studio files
(`%APPDATA%\com.recap.app.dev`), so it can run next to an installed Recap without touching it.

Handy while working on detection without the app:

```powershell
cargo run --release -p recap-core --example scan -- <video.mp4> games/valorant   # detectors on a video
cargo run --release -p recap-core --example read -- <screenshot.png> games/valorant  # screen readers on a frame
cargo test -p recap-core                                                          # core tests
```

The recorder (`crates/recap-recorder`) only records on Windows. Its pure logic (encoder plan,
encoder settings, monitor choice, the command loop) is unit-tested anywhere with
`cargo test -p recap-recorder`; on Linux the OBS backend can still be type-checked:
`LIBOBS_PATH=<any empty dir> cargo clippy -p recap-recorder --features linux-typecheck` (needs libclang).
Its log is `recorder.log` in the data folder: it names the encoder that recorded, any fallback,
buffer splits and capture restarts.

## CI

Every push to `main` builds the installer and runs the end-to-end test on a Windows runner
(about 20 minutes, most of it compiling with no cache warm-up and recording the sample game).
Tagged releases additionally use full link-time optimisation.
