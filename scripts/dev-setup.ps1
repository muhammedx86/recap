# One-time setup for running Recap from source on Windows (see docs/DEVELOPING.md).
# Run from the repository root:  pwsh scripts/dev-setup.ps1
# Safe to run again: finished steps are skipped.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Need([string] $cmd, [string] $how) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        Write-Host "Missing: $cmd. Install it with:`n  $how`nthen open a new terminal and run this script again." -ForegroundColor Yellow
        exit 1
    }
}
Need cargo "winget install Rustlang.Rustup   (then: rustup default stable)"
Need node "winget install OpenJS.NodeJS.LTS"
if (-not (Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe")) {
    Write-Host "Missing: the MSVC C++ build tools. Install with:`n  winget install Microsoft.VisualStudio.2022.BuildTools --override `"--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`"" -ForegroundColor Yellow
    exit 1
}

Write-Host "1/4 UI dependencies" -ForegroundColor Cyan
Push-Location ui; npm ci; Pop-Location

Write-Host "2/4 OBS runtime (libobs) into src-tauri/obs" -ForegroundColor Cyan
if (-not (Get-Command cargo-obs-build -ErrorAction SilentlyContinue)) {
    cargo install cargo-obs-build --version 2.0.3 --locked
}
if (-not (Test-Path src-tauri/obs/obs.dll)) {
    cargo obs-build build --out-dir src-tauri/obs --cache-dir obs-build --remove-pdbs
} else { Write-Host "   already there" }

Write-Host "3/4 ffmpeg + ffprobe into src-tauri/binaries" -ForegroundColor Cyan
if (-not (Test-Path src-tauri/binaries/ffmpeg.exe)) {
    $zip = Join-Path $env:TEMP "recap-ffmpeg.zip"
    $dir = Join-Path $env:TEMP "recap-ffmpeg"
    Invoke-WebRequest -Uri "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" -OutFile $zip
    Expand-Archive $zip -DestinationPath $dir -Force
    $bin = Get-ChildItem $dir -Recurse -Filter ffmpeg.exe | Select-Object -First 1
    New-Item -ItemType Directory -Force -Path src-tauri/binaries | Out-Null
    Copy-Item $bin.FullName src-tauri/binaries/ -Force
    Copy-Item (Join-Path $bin.DirectoryName "ffprobe.exe") src-tauri/binaries/ -Force
} else { Write-Host "   already there" }

Write-Host "4/4 Recorder sidecar (first build takes a few minutes)" -ForegroundColor Cyan
cargo build -p recap-recorder
Copy-Item target/debug/recap-recorder.exe src-tauri/obs/ -Force

Write-Host "`nDone. Start Recap from source with:  pwsh scripts/dev.ps1" -ForegroundColor Green
