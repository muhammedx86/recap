# End-to-end test of the packaged app against the sample game. See docs/E2E.md.
# Usage: pwsh scripts/e2e.ps1 -AppDir portable/Recap -GameExe target/release/recap-sample-game.exe -Out e2e-out
param(
    [Parameter(Mandatory)] [string] $AppDir,
    [Parameter(Mandatory)] [string] $GameExe,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$Out = (New-Item -ItemType Directory -Force -Path $Out).FullName
$AppDir = (Resolve-Path $AppDir).Path
$GameExe = (Resolve-Path $GameExe).Path
$Data = Join-Path $Out "data"
$Recordings = Join-Path $Out "recordings"
$Fixture = Join-Path $Out "fixture"
$Shots = Join-Path $Out "screenshots"
New-Item -ItemType Directory -Force -Path $Data, $Recordings, $Fixture, $Shots | Out-Null

function Shot([string] $name) {
    try {
        $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
        $bmp.Save((Join-Path $Shots "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Write-Host "screenshot: $name ($($b.Width)x$($b.Height))"
    } catch { Write-Warning "screenshot $name failed: $_" }
}

$failures = New-Object System.Collections.Generic.List[string]
function Fail([string] $msg) { $script:failures.Add($msg); Write-Host "FAIL: $msg" -ForegroundColor Red; Write-Host "::error title=E2E::$msg" }
function Pass([string] $msg) { Write-Host "ok:   $msg" -ForegroundColor Green }

# 1. Fixture game + isolated settings.
& $GameExe --export-fixture $Fixture
if ($LASTEXITCODE -ne 0) { throw "fixture export failed" }
$settings = @{
    recordingsDir = $Recordings; fps = 30; bitrateKbps = 4000; encoder = "auto"
    recordDesktopAudio = $false; recordMicrophone = $false
    autoClip = @{ enabled = $true; secondsBefore = 3; secondsAfter = 2; mergeGapSeconds = 1; eventsOverride = @(); preciseCuts = $false }
    scanAfterMatch = $true; splitMatches = $true; analysisHeight = 720
    # CI runners have no GPU: keep x264's load down so capture keeps up.
    outputHeight = 720
}
$settings | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $Data "settings.json")

# 2. Start the app in self-test mode.
$env:RECAP_DATA_DIR = $Data
$env:RECAP_EXTRA_GAMES = $Fixture
$env:RECAP_SELFTEST = $Out
$app = Start-Process -FilePath (Join-Path $AppDir "recap.exe") -WorkingDirectory $AppDir -PassThru
Write-Host "recap.exe pid $($app.Id)"
Start-Sleep -Seconds 15
Shot "00-app-started"
if ($app.HasExited) { Fail "recap.exe exited early with code $($app.ExitCode)" }
else {
    # 3. Play the sample game; Recap should detect, record and stop on its own.
    # The game window is 1280x720; on a smaller screen shrink it (16:9 kept) so Windows does not squeeze it.
    $wa = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $scale = [math]::Min(1.0, [math]::Min(($wa.Width - 40) / 1280.0, ($wa.Height - 60) / 720.0))
    $gameArgs = if ($scale -lt 1.0) { @("--window-scale", ("{0:N3}" -f $scale)) } else { @() }
    Write-Host "screen $($wa.Width)x$($wa.Height) working area; game window scale $scale"
    $game = if ($gameArgs.Count) { Start-Process -FilePath $GameExe -ArgumentList $gameArgs -PassThru } else { Start-Process -FilePath $GameExe -PassThru }
    Write-Host "sample game pid $($game.Id)"
    Start-Sleep -Seconds 20
    Shot "01-recording-sample-game"
    if (-not $game.WaitForExit(120000)) { Fail "sample game did not exit"; Stop-Process -Id $game.Id -Force }
    Start-Sleep -Seconds 3
    Shot "02-after-game"
}

# 4. Wait for processing + the self-test check suite.
$reportPath = Join-Path $Out "report.json"
$deadline = (Get-Date).AddMinutes(6)
while (-not (Test-Path $reportPath) -and (Get-Date) -lt $deadline -and -not $app.HasExited) { Start-Sleep -Seconds 3 }

# 5. Screenshot the UI tour.
$tour = Join-Path $Out "tour.txt"
$seen = @{}
$tourDeadline = (Get-Date).AddMinutes(2)
while ((Test-Path $reportPath) -and (Get-Date) -lt $tourDeadline -and -not $app.HasExited) {
    if (Test-Path $tour) {
        $page = (Get-Content $tour -Raw).Trim()
        if ($page -eq "done") { break }
        if ($page -and -not $seen.ContainsKey($page)) { $seen[$page] = $true; Shot "page-$page" }
    }
    Start-Sleep -Milliseconds 500
}

# 5b. Phase 2: play again and press "stop" during match 1. The rest of match 1 must be skipped
#     and match 2 recorded on its own, without relaunching anything.
$report2Path = Join-Path $Out "report2.json"
if ((Test-Path $reportPath) -and -not $app.HasExited) {
    $r1 = Get-Content $reportPath -Raw | ConvertFrom-Json
    ($r1.matches | ForEach-Object { $_.id }) -join "`n" | Set-Content (Join-Path $Out "phase2.txt")
    $game2 = if ($gameArgs.Count) { Start-Process -FilePath $GameExe -ArgumentList $gameArgs -PassThru } else { Start-Process -FilePath $GameExe -PassThru }
    $t0 = Get-Date
    Write-Host "phase 2: sample game pid $($game2.Id)"
    while (((Get-Date) - $t0).TotalSeconds -lt 18) { Start-Sleep -Milliseconds 200 }
    Set-Content (Join-Path $Out "stop.request") "stop"
    Write-Host "phase 2: manual stop at +18 s (mid match 1)"
    Start-Sleep -Seconds 12
    Shot "03-phase2-after-stop"
    if (-not $game2.WaitForExit(120000)) { Fail "phase 2 game did not exit"; Stop-Process -Id $game2.Id -Force }
    Set-Content (Join-Path $Out "phase2-game-exited") "1"
    $deadline2 = (Get-Date).AddMinutes(5)
    while (-not (Test-Path $report2Path) -and (Get-Date) -lt $deadline2 -and -not $app.HasExited) { Start-Sleep -Seconds 3 }
}

# 6. Stop the app and collect logs.
if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }
Get-Process recap-recorder -ErrorAction SilentlyContinue | Stop-Process -Force
Get-ChildItem $Data -Filter *.log -Recurse | Copy-Item -Destination $Out -Force
# Make the key log lines visible in the CI log itself.
foreach ($l in Get-ChildItem $Out -Filter *.log) {
    Write-Host "----- $($l.Name) (last 60 lines) -----"
    Get-Content $l.FullName -Tail 60 | ForEach-Object { Write-Host $_ }
}
Get-ChildItem $Recordings -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "recording: $($_.FullName) $($_.Length) bytes" }

# 7. Assertions.
if (-not (Test-Path $reportPath)) {
    Fail "no report.json (processing never finished; see recap.log / recorder.log)"
} else {
    $r = Get-Content $reportPath -Raw | ConvertFrom-Json

    foreach ($c in $r.checks.PSObject.Properties) {
        if ($c.Value.ok) { Pass "check $($c.Name)" } else { Fail "check $($c.Name): $($c.Value.detail)" }
    }

    $sample = @($r.matches | Where-Object { $_.gameId -eq "recap-sample-game" })
    if ($sample.Count -ne 2) { Fail "expected 2 sample matches after splitting, got $($sample.Count)" } else { Pass "session split into 2 matches" }
    foreach ($m in $r.matches) {
        if ($m.status -ne "ready") { Fail "match $($m.id) status $($m.status): $($m.error)" }
        if (-not $m.exists) { Fail "match file missing: $($m.path)" }
    }

    function Expect($m, [string] $label, [string] $kind, [double[]] $want) {
        $start = @($m.events | Where-Object kind -eq "match_start" | Sort-Object tMs)[0]
        if (-not $start) { Fail "$label has no match_start event"; return }
        [double[]] $got = @($m.events | Where-Object kind -eq $kind | Sort-Object tMs | ForEach-Object { ($_.tMs - $start.tMs) / 1000.0 })
        $ok = $got.Count -eq $want.Count
        if ($ok) { for ($i = 0; $i -lt $want.Count; $i++) { if ([math]::Abs($got[$i] - $want[$i]) -gt 1.5) { $ok = $false } } }
        $g = ($got | ForEach-Object { "{0:N1}" -f $_ }) -join ", "
        if ($ok) { Pass "$label $kind at +[$g] s" } else { Fail "$label $kind expected +[$($want -join ', ')] s, got +[$g] s" }
    }
    if ($sample.Count -ge 1) {
        $m1 = $sample[0]
        Expect $m1 "match 1" "kill" @(5, 8, 14)
        Expect $m1 "match 1" "round_start" @(2.5, 12)
        Expect $m1 "match 1" "round_win" @(10)
        Expect $m1 "match 1" "round_loss" @(16.5)
        Expect $m1 "match 1" "match_end" @(19)
    }
    if ($sample.Count -ge 2) {
        $m2 = $sample[1]
        Expect $m2 "match 2" "kill" @(5)
        Expect $m2 "match 2" "round_start" @(2.5)
        Expect $m2 "match 2" "round_win" @(8)
        Expect $m2 "match 2" "match_end" @(11)
    }
    foreach ($m in $sample) {
        $auto = @($m.clips | Where-Object { $_.kind -eq "auto" -and $_.exists })
        if ($auto.Count -ge 1) { Pass "auto-clips for $($m.id): $($auto.Count)" } else { Fail "no auto-clips for match $($m.id)" }
    }
    # Match details read from the sample game's end screens (see crates/sample-game/src/endscreen.rs).
    function ExpectDetails($m, [string] $label, [hashtable] $want) {
        foreach ($f in $want.Keys) {
            $got = $m.details.$f
            $val = if ($got) { [string]$got.value } else { "" }
            $src = if ($got) { $got.source } else { "-" }
            if ($val -eq $want[$f]) { Pass "$label $f = $val ($src)" }
            elseif (-not $r.ocrAvailable -and $f -ne "character") { Fail "$label $f missing: Windows OCR is not available on this machine" }
            else { Fail "$label $f expected '$($want[$f])', got '$val' ($src)" }
        }
    }
    if ($sample.Count -ge 1) {
        ExpectDetails $sample[0] "match 1" @{ character = "Ring"; map = "Harbor"; kills = "3"; deaths = "1"; assists = "2"; result = "win"; score = "13-11" }
    }
    if ($sample.Count -ge 2) {
        ExpectDetails $sample[1] "match 2" @{ character = "Cross"; map = "Canyon"; kills = "1"; deaths = "0"; assists = "4"; result = "loss"; score = "9-13" }
    }

    $mont = @($r.montages | Where-Object exists)
    if ($mont.Count -ge 1) { Pass "montage written" } else { Fail "no montage file" }
}
# Phase 2 assertions.
if (-not (Test-Path $report2Path)) {
    Fail "no report2.json (phase 2: manual stop mid-match did not finish)"
} else {
    $r2 = Get-Content $report2Path -Raw | ConvertFrom-Json
    $new = @($r2.matches)
    Write-Host "phase 2 recordings: $($new.Count)"
    foreach ($m in $new) { Write-Host ("  {0} ms: {1}" -f $m.durationMs, (($m.events | ForEach-Object { "$($_.kind)@$([math]::Round($_.tMs/1000,1))" }) -join ", ")) }
    $full = @($new | Where-Object { @($_.events | Where-Object kind -eq "round_win").Count -ge 1 -and @($_.events | Where-Object kind -eq "match_end").Count -ge 1 })
    $skipped = @($new | Where-Object { @($_.events | Where-Object kind -eq "round_loss").Count -ge 1 })
    if ($full.Count -eq 1) { Pass "phase 2: the next match recorded on its own after a manual stop" } else { Fail "phase 2: expected 1 full match after the stop, got $($full.Count)" }
    if ($skipped.Count -eq 0) { Pass "phase 2: the rest of the stopped match was skipped" } else { Fail "phase 2: the skipped match's remainder (round lost at 26.5 s) was kept" }
    if ($full.Count -eq 1) {
        $m2 = $full[0]
        $starts = @($m2.events | Where-Object kind -eq "match_start")
        $kills = @($m2.events | Where-Object kind -eq "kill")
        if ($starts.Count -eq 1 -and $kills.Count -eq 1) { Pass "phase 2 match is match 2 (1 start, 1 kill)" } else { Fail "phase 2 match has $($starts.Count) starts and $($kills.Count) kills (expected 1 and 1)" }
    }
}
foreach ($p in "library", "match", "clips", "studio", "games", "settings") {
    if (-not (Test-Path (Join-Path $Shots "page-$p.png"))) { Fail "no screenshot of the $p page (UI tour did not run)" }
}

$summary = if ($failures.Count) { "E2E FAILED ($($failures.Count)):`n" + ($failures -join "`n") } else { "E2E PASSED" }
$summary | Set-Content (Join-Path $Out "summary.txt")
Write-Host $summary
if ($failures.Count) { exit 1 }
