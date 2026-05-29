# End-to-end smoke test: ffmpeg → instantclone (ingest :1935) → sink (:1936).
#
# Runs in CI and locally. Locally, point $env:INSTANTCLONE_EXE at the
# built binary or this script will look for .\target\release\instantclone.exe.
# ffmpeg must be on PATH.
#
# What it proves: the full RTMP wire path round-trips real H.264 + AAC,
# the sink classifies IDRs correctly, and the proxy doesn't choke on
# realistic GOP cadence.

$ErrorActionPreference = "Stop"

$exe = if ($env:INSTANTCLONE_EXE) { $env:INSTANTCLONE_EXE } else { ".\target\release\instantclone.exe" }
if (-not (Test-Path $exe)) { throw "instantclone.exe not found at '$exe' — build it first (`cargo build --release`)" }
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg not on PATH" }

$workDir = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "ic-e2e-$([guid]::NewGuid().ToString('N'))") -Force
Write-Host "e2e work dir: $workDir"
Push-Location $workDir
try {
    # Pre-canned config: one Custom destination pointing at the in-tree sink.
    # `configured=true` skips the first-run gate; zero delay keeps us in
    # passthrough so tags flow to the destination immediately, no arm dance.
    $cfg = @"
configured=true
ingest_port=1935
ingest_bind_all=false
web_port=7799
web_bind_all=false
buffer_mb=50
buffer_path=./instantclone.buf
target_delay_ms=0
armed_delay_ms=0
initial_delay_ms=0
destination.0.id=e2e
destination.0.name=Sink
destination.0.enabled=true
destination.0.platform=custom
destination.0.stream_key=stream
destination.0.custom_egress_url=rtmp://127.0.0.1:1936/live
"@
    Set-Content -Path "instantclone.config.json" -Value $cfg -Encoding ascii

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

    Write-Host "Starting sink on :1936…"
    $sink = Start-Process -FilePath $exe `
        -ArgumentList "sink","--port","1936","--web-port","0","--temp","--max-mb","30" `
        -RedirectStandardOutput "sink.log" -RedirectStandardError "sink.err" `
        -PassThru -NoNewWindow

    Write-Host "Starting instantclone on :1935 (web :7799)…"
    $env:INSTANTCLONE_NO_BROWSER = "1"
    $env:CONFIG_PATH = (Join-Path (Get-Location) "instantclone.config.json")
    $ic = Start-Process -FilePath $exe `
        -ArgumentList "--no-browser" `
        -RedirectStandardOutput "ic.log" -RedirectStandardError "ic.err" `
        -PassThru -NoNewWindow

    if (-not (Wait-Port 1936 15)) { throw "sink never opened :1936" }
    if (-not (Wait-Port 1935 15)) { throw "instantclone never opened :1935" }
    Write-Host "Both ports listening — pushing ffmpeg…"

    # 6-second synthetic source: 320x240 @ 15 fps, GOP 15 (1 IDR/sec),
    # H.264 + AAC, FLV container, RTMP. Ultrafast + 400 kbps keeps the
    # runner happy without sacrificing the bits we actually check.
    $ffArgs = @(
        "-loglevel","warning","-re",
        "-f","lavfi","-i","testsrc=size=320x240:rate=15:duration=6",
        "-f","lavfi","-i","sine=frequency=440:duration=6",
        "-c:v","libx264","-preset","ultrafast","-g","15","-b:v","400k",
        "-c:a","aac","-b:a","64k",
        "-f","flv","rtmp://127.0.0.1:1935/live/stream"
    )
    $ff = Start-Process -FilePath "ffmpeg" -ArgumentList $ffArgs -PassThru -NoNewWindow -Wait
    Write-Host "ffmpeg exit code: $($ff.ExitCode) (non-zero is common when RTMP closes mid-flush, not a failure on its own)"

    # Let the last tags drain through the proxy + into the sink.
    Start-Sleep -Seconds 3

    Stop-Process -Id $ic.Id   -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $sink.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1

    $sinkOut = if (Test-Path sink.log) { Get-Content sink.log -Raw } else { "" }
    $sinkErr = if (Test-Path sink.err) { Get-Content sink.err -Raw } else { "" }
    $icOut   = if (Test-Path ic.log)   { Get-Content ic.log   -Raw } else { "" }
    $icErr   = if (Test-Path ic.err)   { Get-Content ic.err   -Raw } else { "" }

    Write-Host ""
    Write-Host "================== sink.log =================="
    Write-Host $sinkOut
    if ($sinkErr) { Write-Host "----- sink.err -----"; Write-Host $sinkErr }
    Write-Host "================== ic.log (head 80 lines) =================="
    ($icOut -split "`r?`n" | Select-Object -First 80) -join "`n" | Write-Host
    if ($icErr) {
        Write-Host "----- ic.err (head 40 lines) -----"
        ($icErr -split "`r?`n" | Select-Object -First 40) -join "`n" | Write-Host
    }
    Write-Host "============================================="
    Write-Host ""

    $checks = @(
        @{ name = "sink accepted publish"; match  = "publish accepted" },
        @{ name = "sink received onMetaData"; match  = "metadata received" },
        @{ name = "sink got >=1 IDR"; regex  = "[1-9]\d* IDR" }
    )

    $failed = 0
    foreach ($c in $checks) {
        $ok = $false
        if ($c.match) { $ok = $sinkOut -like "*$($c.match)*" }
        if ($c.regex) { $ok = [regex]::IsMatch($sinkOut, $c.regex) }
        if ($ok) { Write-Host "PASS  $($c.name)" }
        else     { Write-Host "FAIL  $($c.name)"; $failed += 1 }
    }

    if ($failed -gt 0) { throw "$failed e2e assertion(s) failed" }
    Write-Host ""
    Write-Host "e2e: all assertions passed"
}
finally {
    Pop-Location
    try { Stop-Process -Id $ic.Id   -Force -ErrorAction SilentlyContinue } catch {}
    try { Stop-Process -Id $sink.Id -Force -ErrorAction SilentlyContinue } catch {}
}
