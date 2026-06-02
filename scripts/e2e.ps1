# End-to-end smoke tests: ffmpeg -> instantclone (ingest :1935) -> sink(s).
#
# Runs in CI and locally. Locally, point $env:INSTANTCLONE_EXE at the
# built binary or this script will look for .\target\release\instantclone.exe.
# ffmpeg must be on PATH.
#
# Scenarios exercised:
#
#   A. Basic passthrough - one sink, one publisher, IDR + audio land.
#      The historical smoke test.
#
#   B. Publisher reconnect - ffmpeg cuts mid-flight + reconnects. The
#      sink-side `publish accepted` count must reach 2 and audio +
#      IDR counts must continue across the cut. This is the scenario
#      that caught the EB seq-header-leak regression: a stale cache
#      from session A would inject bad config bytes into session B,
#      visible here as the sink dropping the second connection.
#
#   C. Multi-destination - one publisher, two parallel sinks, both
#      receive a clean stream. Catches per-destination state
#      regressions and consumer_seq pinning bugs that would stop the
#      ring from trimming.
#
#   D. HTTP API smoke - arm -> state -> disarm -> state -> reset ->
#      state. Each call must return 200 and the JSON shape must
#      include the keys the dashboard reads (phase, ingest_alive,
#      destinations, stats). Catches accidental endpoint removals
#      and dashboard-server contract drift.

$ErrorActionPreference = "Stop"

$exe = if ($env:INSTANTCLONE_EXE) { $env:INSTANTCLONE_EXE } else { ".\target\release\instantclone.exe" }
if (-not (Test-Path $exe)) { throw "instantclone.exe not found at '$exe' - build it first (`cargo build --release`)" }
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg not on PATH" }

# --- Helpers ----------------------------------------------------------

function Wait-Port($port, $timeoutSec) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $timeoutSec) {
        try {
            $c = New-Object System.Net.Sockets.TcpClient
            $iar = $c.BeginConnect("127.0.0.1", $port, $null, $null)
            $ok = $iar.AsyncWaitHandle.WaitOne(500)
            if ($ok -and $c.Connected) { $c.Close(); return $true }
            $c.Close()
        } catch {}
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Wait-Http($url, $timeoutSec) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $timeoutSec) {
        try {
            $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            if ($r.StatusCode -eq 200) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Stop-Safe($proc) {
    if ($null -eq $proc) { return }
    try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue } catch {}
}

function Push-FfmpegSource($durationSec, $bitrateK = 400) {
    # Synthetic source: 320x240 @ 15 fps, GOP 15 (1 IDR/sec),
    # H.264 + AAC, FLV container, RTMP. Returns the exit code.
    $ffArgs = @(
        "-loglevel","error","-re",
        "-f","lavfi","-i","testsrc=size=320x240:rate=15:duration=$durationSec",
        "-f","lavfi","-i","sine=frequency=440:duration=$durationSec",
        "-c:v","libx264","-preset","ultrafast","-g","15","-b:v","${bitrateK}k",
        "-c:a","aac","-b:a","64k",
        "-f","flv","rtmp://127.0.0.1:1935/live/stream"
    )
    $ff = Start-Process -FilePath "ffmpeg" -ArgumentList $ffArgs -PassThru -NoNewWindow -Wait
    return $ff.ExitCode
}

# Track per-scenario results. Each scenario appends { name; failed[] }.
$script:report = @()

function Run-Scenario($name, $block) {
    Write-Host ""
    Write-Host "==========================================================="
    Write-Host "> Scenario: $name"
    Write-Host "==========================================================="
    $failed = New-Object System.Collections.ArrayList
    try {
        & $block ([ref]$failed)
    } catch {
        [void]$failed.Add("scenario threw: $($_.Exception.Message)")
    }
    if ($failed.Count -eq 0) {
        Write-Host "PASS  $name" -ForegroundColor Green
    } else {
        Write-Host "FAIL  $name" -ForegroundColor Red
        foreach ($f in $failed) { Write-Host "      * $f" -ForegroundColor Red }
    }
    $script:report += @{ name = $name; failed = $failed }
}

function Assert($cond, $why, [ref]$failed) {
    if (-not $cond) { [void]$failed.Value.Add($why) }
}

# Build the standard wizard-skipping config. $destBlocks is an array of
# hashtables, one per destination, with keys id/name/platform/url/key.
function Write-Config($destBlocks) {
    $lines = @(
        "configured=true",
        "ingest_port=1935",
        "ingest_bind_all=false",
        "web_port=7799",
        "web_bind_all=false",
        "buffer_mb=50",
        "buffer_path=./instantclone.buf",
        "target_delay_ms=0",
        "armed_delay_ms=0",
        "initial_delay_ms=0"
    )
    for ($i = 0; $i -lt $destBlocks.Count; $i++) {
        $d = $destBlocks[$i]
        $lines += @(
            "destination.$i.id=$($d.id)",
            "destination.$i.name=$($d.name)",
            "destination.$i.enabled=true",
            "destination.$i.platform=$($d.platform)",
            "destination.$i.stream_key=$($d.key)",
            "destination.$i.custom_egress_url=$($d.url)"
        )
    }
    Set-Content -Path "instantclone.config.json" -Value ($lines -join "`n") -Encoding ascii
}

$workDir = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "ic-e2e-$([guid]::NewGuid().ToString('N'))") -Force
Write-Host "e2e work dir: $workDir"
Push-Location $workDir

try {

# --- Scenario A: basic passthrough -----------------------------------

Run-Scenario "A -basic passthrough (1 sink, 1 publisher)" {
    param([ref]$failed)
    Write-Config @(@{ id="e2e"; name="Sink"; platform="custom"; url="rtmp://127.0.0.1:1936/live"; key="stream" })
    $sink = Start-Process -FilePath $exe -ArgumentList "sink","--port","1936","--web-port","0","--temp","--max-mb","30" `
        -RedirectStandardOutput "A.sink.log" -RedirectStandardError "A.sink.err" -PassThru -NoNewWindow
    $env:INSTANTCLONE_NO_BROWSER = "1"
    $env:CONFIG_PATH = (Join-Path (Get-Location) "instantclone.config.json")
    $ic = Start-Process -FilePath $exe -ArgumentList "--no-browser" `
        -RedirectStandardOutput "A.ic.log" -RedirectStandardError "A.ic.err" -PassThru -NoNewWindow
    try {
        Assert (Wait-Port 1936 15) "sink never opened :1936" $failed
        Assert (Wait-Port 1935 15) "instantclone never opened :1935" $failed
        if ($failed.Value.Count -gt 0) { return }
        [void](Push-FfmpegSource 6)
        Start-Sleep -Seconds 3
        $sinkOut = Get-Content "A.sink.log" -Raw
        Assert ($sinkOut -like "*publish accepted*") "sink never accepted publish" $failed
        Assert ([regex]::IsMatch($sinkOut, "[1-9]\d* IDR")) "sink saw zero IDRs" $failed
        Assert ([regex]::IsMatch($sinkOut, "audio=[1-9]\d*")) "sink saw zero audio frames" $failed
    } finally {
        Stop-Safe $ic; Stop-Safe $sink
        Start-Sleep -Seconds 1
    }
}

# --- Scenario B: publisher reconnect ---------------------------------

Run-Scenario "B -publisher reconnect (catches seq-header leak)" {
    param([ref]$failed)
    Write-Config @(@{ id="e2e"; name="Sink"; platform="custom"; url="rtmp://127.0.0.1:1936/live"; key="stream" })
    $sink = Start-Process -FilePath $exe -ArgumentList "sink","--port","1936","--web-port","0","--temp","--max-mb","30" `
        -RedirectStandardOutput "B.sink.log" -RedirectStandardError "B.sink.err" -PassThru -NoNewWindow
    $env:INSTANTCLONE_NO_BROWSER = "1"
    $env:CONFIG_PATH = (Join-Path (Get-Location) "instantclone.config.json")
    $ic = Start-Process -FilePath $exe -ArgumentList "--no-browser" `
        -RedirectStandardOutput "B.ic.log" -RedirectStandardError "B.ic.err" -PassThru -NoNewWindow
    try {
        Assert (Wait-Port 1936 15) "sink never opened :1936" $failed
        Assert (Wait-Port 1935 15) "instantclone never opened :1935" $failed
        if ($failed.Value.Count -gt 0) { return }
        # First session: 3 s of stream.
        [void](Push-FfmpegSource 3)
        Start-Sleep -Seconds 2
        # Cut. Wait for ingest to mark dead, give the supervisor a tick.
        Start-Sleep -Seconds 2
        # Second session: another 3 s. Must reach the sink cleanly.
        [void](Push-FfmpegSource 3)
        Start-Sleep -Seconds 3
        $sinkOut = Get-Content "B.sink.log" -Raw
        $accepts = ([regex]::Matches($sinkOut, "publish accepted")).Count
        Assert ($accepts -ge 2) "sink only saw $accepts publish accepts (expected >= 2)" $failed
        Assert ([regex]::IsMatch($sinkOut, "[1-9]\d* IDR")) "sink saw zero IDRs across the two sessions" $failed
    } finally {
        Stop-Safe $ic; Stop-Safe $sink
        Start-Sleep -Seconds 1
    }
}

# --- Scenario C: multi-destination ------------------------------------

Run-Scenario "C -multi-destination (1 publisher, 2 sinks)" {
    param([ref]$failed)
    Write-Config @(
        @{ id="d1"; name="SinkOne"; platform="custom"; url="rtmp://127.0.0.1:1936/live"; key="s1" },
        @{ id="d2"; name="SinkTwo"; platform="custom"; url="rtmp://127.0.0.1:1937/live"; key="s2" }
    )
    $sink1 = Start-Process -FilePath $exe -ArgumentList "sink","--port","1936","--web-port","0","--temp","--max-mb","30" `
        -RedirectStandardOutput "C.sink1.log" -RedirectStandardError "C.sink1.err" -PassThru -NoNewWindow
    $sink2 = Start-Process -FilePath $exe -ArgumentList "sink","--port","1937","--web-port","0","--temp","--max-mb","30" `
        -RedirectStandardOutput "C.sink2.log" -RedirectStandardError "C.sink2.err" -PassThru -NoNewWindow
    $env:INSTANTCLONE_NO_BROWSER = "1"
    $env:CONFIG_PATH = (Join-Path (Get-Location) "instantclone.config.json")
    $ic = Start-Process -FilePath $exe -ArgumentList "--no-browser" `
        -RedirectStandardOutput "C.ic.log" -RedirectStandardError "C.ic.err" -PassThru -NoNewWindow
    try {
        Assert (Wait-Port 1936 15) "sink1 never opened :1936" $failed
        Assert (Wait-Port 1937 15) "sink2 never opened :1937" $failed
        Assert (Wait-Port 1935 15) "instantclone never opened :1935" $failed
        if ($failed.Value.Count -gt 0) { return }
        [void](Push-FfmpegSource 6)
        Start-Sleep -Seconds 3
        $s1 = Get-Content "C.sink1.log" -Raw
        $s2 = Get-Content "C.sink2.log" -Raw
        Assert ($s1 -like "*publish accepted*") "sink1 never accepted publish" $failed
        Assert ($s2 -like "*publish accepted*") "sink2 never accepted publish" $failed
        Assert ([regex]::IsMatch($s1, "[1-9]\d* IDR")) "sink1 saw zero IDRs" $failed
        Assert ([regex]::IsMatch($s2, "[1-9]\d* IDR")) "sink2 saw zero IDRs" $failed
    } finally {
        Stop-Safe $ic; Stop-Safe $sink1; Stop-Safe $sink2
        Start-Sleep -Seconds 1
    }
}

# --- Scenario D: HTTP API smoke ---------------------------------------

Run-Scenario "D -HTTP API smoke (arm/state/disarm/reset)" {
    param([ref]$failed)
    Write-Config @(@{ id="e2e"; name="Sink"; platform="custom"; url="rtmp://127.0.0.1:1936/live"; key="stream" })
    $env:INSTANTCLONE_NO_BROWSER = "1"
    $env:CONFIG_PATH = (Join-Path (Get-Location) "instantclone.config.json")
    $ic = Start-Process -FilePath $exe -ArgumentList "--no-browser" `
        -RedirectStandardOutput "D.ic.log" -RedirectStandardError "D.ic.err" -PassThru -NoNewWindow
    try {
        Assert (Wait-Http "http://127.0.0.1:7799/state" 15) "web UI never came up on :7799" $failed
        if ($failed.Value.Count -gt 0) { return }

        # GET /state - must include the keys the dashboard reads.
        $state = (Invoke-RestMethod "http://127.0.0.1:7799/state" -TimeoutSec 5)
        foreach ($k in @("phase","ingest_alive","destinations","stats")) {
            Assert ($null -ne $state.$k -or $state.PSObject.Properties.Name -contains $k) "GET /state missing key '$k'" $failed
        }

        # POST /arm ms=2000 - phase must leave idle.
        $arm = Invoke-RestMethod "http://127.0.0.1:7799/arm" -Method POST -Body "ms=2000" -ContentType "application/x-www-form-urlencoded" -TimeoutSec 5
        Start-Sleep -Milliseconds 300
        $state = (Invoke-RestMethod "http://127.0.0.1:7799/state" -TimeoutSec 5)
        Assert ($state.phase -ne "idle") "phase still 'idle' after /arm ms=2000 (got '$($state.phase)')" $failed

        # POST /disarm - phase must return to idle.
        Invoke-RestMethod "http://127.0.0.1:7799/disarm" -Method POST -TimeoutSec 5 | Out-Null
        Start-Sleep -Milliseconds 300
        $state = (Invoke-RestMethod "http://127.0.0.1:7799/state" -TimeoutSec 5)
        Assert ($state.phase -eq "idle") "phase not 'idle' after /disarm (got '$($state.phase)')" $failed

        # POST /config/reset?scope=settings - destinations must survive.
        Invoke-RestMethod "http://127.0.0.1:7799/config/reset?scope=settings" -Method POST -TimeoutSec 5 | Out-Null
        Start-Sleep -Milliseconds 300
        $cfg = (Invoke-RestMethod "http://127.0.0.1:7799/config" -TimeoutSec 5)
        Assert ($cfg.ingest_port -eq 1935) "ingest_port not reset to default (got $($cfg.ingest_port))" $failed
        Assert ($cfg.web_port -eq 7799) "web_port not reset to default (got $($cfg.web_port))" $failed
        Assert ($cfg.destinations.Count -ge 1) "destinations wiped by scope=settings reset (got $($cfg.destinations.Count))" $failed

        # POST /config/reset?scope=all - destinations must be wiped + configured=false.
        Invoke-RestMethod "http://127.0.0.1:7799/config/reset?scope=all" -Method POST -TimeoutSec 5 | Out-Null
        Start-Sleep -Milliseconds 300
        $cfg = (Invoke-RestMethod "http://127.0.0.1:7799/config" -TimeoutSec 5)
        Assert ($cfg.destinations.Count -eq 0) "destinations not wiped by scope=all reset (got $($cfg.destinations.Count))" $failed
        Assert ($cfg.configured -eq $false) "configured not flipped to false by scope=all reset" $failed
    } finally {
        Stop-Safe $ic
        Start-Sleep -Seconds 1
    }
}

# --- Report -----------------------------------------------------------

Write-Host ""
Write-Host "==========================================================="
Write-Host "  E2E REPORT"
Write-Host "==========================================================="
$totalFailed = 0
foreach ($r in $script:report) {
    if ($r.failed.Count -eq 0) {
        Write-Host "  PASS  $($r.name)" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $($r.name)" -ForegroundColor Red
        foreach ($f in $r.failed) { Write-Host "        * $f" -ForegroundColor Red }
        $totalFailed += $r.failed.Count
    }
}
Write-Host ""
if ($totalFailed -gt 0) { throw "$totalFailed e2e assertion(s) failed across $($script:report.Count) scenario(s)" }
Write-Host "e2e: all $($script:report.Count) scenarios passed" -ForegroundColor Green

}
finally {
    Pop-Location
}
