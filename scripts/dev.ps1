# Run Recap from source with live reload (see docs/DEVELOPING.md).
#   pwsh scripts/dev.ps1            the app; UI edits reload instantly, Rust edits rebuild + restart
#   pwsh scripts/dev.ps1 -UiOnly    just the UI in a browser with demo data (no game, no recorder)
#   pwsh scripts/dev.ps1 -Release   optimised build (closer to the installer, slower to build)
param([switch] $UiOnly, [switch] $Release)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if ($UiOnly) {
    Push-Location ui
    Write-Host "Open http://localhost:1420 (demo data)." -ForegroundColor Green
    npm run dev
    Pop-Location
    return
}

if (-not (Test-Path src-tauri/obs/obs.dll) -or -not (Test-Path src-tauri/binaries/ffmpeg.exe)) {
    Write-Host "Run the one-time setup first:  pwsh scripts/dev-setup.ps1" -ForegroundColor Yellow
    exit 1
}

# The recorder is a separate program: rebuild it if its code changed.
$buildProfile = if ($Release) { "release" } else { "debug" }
if ($Release) { cargo build -p recap-recorder --release } else { cargo build -p recap-recorder }
Copy-Item "target/$buildProfile/recap-recorder.exe" src-tauri/obs/ -Force

# A separate identity, so this runs next to an installed Recap with its own settings and library
# (single-instance and app data are keyed by the identifier).
$env:TAURI_FRONTEND_PATH = "ui"
$config = '{"identifier":"com.recap.app.dev","productName":"Recap (dev)"}'
$cliArgs = @("--yes", "@tauri-apps/cli@2", "dev", "--config", $config)
if ($Release) { $cliArgs += "--release" }
npx @cliArgs
