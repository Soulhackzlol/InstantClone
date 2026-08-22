#!/usr/bin/env bash
# End-to-end smoke tests: ffmpeg -> instantclone (ingest :1935) -> sink(s).
# Linux port of e2e.ps1 - same six scenarios, so the wire path (handshake,
# AMF0, chunking, h264 classification, delay+cut) is proven on Linux too.
#
# Runs in CI and locally. Locally, point $INSTANTCLONE_EXE at the built binary
# or this script looks for ./target/release/instantclone. Needs ffmpeg + jq
# + curl on PATH (all preinstalled on GitHub's ubuntu-latest).
#
# Scenarios: A basic passthrough, B publisher reconnect (seq-header leak),
# C multi-destination, D HTTP API smoke, E delay + IDR-aligned cut,
# F scheduled "cut after this airs".

set -u

EXE="${INSTANTCLONE_EXE:-./target/release/instantclone}"
[ -x "$EXE" ] || { echo "instantclone not found/executable at '$EXE' - build it first (cargo build --release)"; exit 1; }
# Pin EXE to an absolute path now: each scenario cd's into a fresh temp work
# dir, so the relative default would stop resolving once the cwd changes.
EXE="$(cd "$(dirname "$EXE")" && pwd)/$(basename "$EXE")"
for tool in ffmpeg jq curl; do
  command -v "$tool" >/dev/null || { echo "$tool not on PATH"; exit 1; }
done

# --- Helpers ----------------------------------------------------------

wait_port() { # host-less: $1=port $2=timeout_sec
  local end=$((SECONDS + $2))
  while [ $SECONDS -lt $end ]; do
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && { exec 3>&- 3<&-; return 0; }
    sleep 0.25
  done
  return 1
}

wait_http() { # $1=url $2=timeout_sec
  local end=$((SECONDS + $2))
  while [ $SECONDS -lt $end ]; do
    [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$1")" = "200" ] && return 0
    sleep 0.25
  done
  return 1
}

get_state() { curl -s --max-time 2 http://127.0.0.1:7799/state; }

# Poll GET /state until the jq boolean filter in $1 is true (or timeout $2).
wait_state() {
  local end=$((SECONDS + $2)) s
  while [ $SECONDS -lt $end ]; do
    s=$(get_state)
    [ -n "$s" ] && [ "$(echo "$s" | jq -r "$1" 2>/dev/null)" = "true" ] && return 0
    sleep 0.3
  done
  return 1
}

stop_safe() { [ -n "${1:-}" ] && kill "$1" 2>/dev/null; wait "$1" 2>/dev/null; return 0; }

push_ffmpeg() { # $1=duration_sec ; blocking
  ffmpeg -loglevel error -re \
    -f lavfi -i "testsrc=size=320x240:rate=15:duration=$1" \
    -f lavfi -i "sine=frequency=440:duration=$1" \
    -c:v libx264 -preset ultrafast -g 15 -b:v 400k \
    -c:a aac -b:a 64k \
    -f flv rtmp://127.0.0.1:1935/live/stream >/dev/null 2>&1
}

start_ffmpeg() { # $1=duration_sec ; non-blocking, echoes PID
  ffmpeg -loglevel error -re \
    -f lavfi -i "testsrc=size=320x240:rate=15:duration=$1" \
    -f lavfi -i "sine=frequency=440:duration=$1" \
    -c:v libx264 -preset ultrafast -g 15 -b:v 400k \
    -c:a aac -b:a 64k \
    -f flv rtmp://127.0.0.1:1935/live/stream >ffsrc.out 2>ffsrc.err &
  echo $!
}

start_sink() { # $1=port $2=logfile ; echoes PID
  "$EXE" sink --port "$1" --web-port 0 --temp --max-mb 50 >"$2" 2>&1 &
  echo $!
}

start_ic() { # $1=logfile ; echoes PID
  INSTANTCLONE_NO_BROWSER=1 CONFIG_PATH="$PWD/instantclone.config.json" \
    "$EXE" --no-browser >"$1" 2>&1 &
  echo $!
}

# write_config "id|name|platform|url|key" ["..."]...
write_config() {
  {
    cat <<'EOF'
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
EOF
    local i=0 d id name platform url key
    for d in "$@"; do
      IFS='|' read -r id name platform url key <<<"$d"
      printf 'destination.%s.id=%s\n' "$i" "$id"
      printf 'destination.%s.name=%s\n' "$i" "$name"
      printf 'destination.%s.enabled=true\n' "$i"
      printf 'destination.%s.platform=%s\n' "$i" "$platform"
      printf 'destination.%s.stream_key=%s\n' "$i" "$key"
      printf 'destination.%s.custom_egress_url=%s\n' "$i" "$url"
      i=$((i + 1))
    done
  } >instantclone.config.json
}

# --- Scenario harness -------------------------------------------------

FAILS=()        # failures for the current scenario
PASS_COUNT=0
FAIL_COUNT=0
REPORT=()

fail() { FAILS+=("$1"); }
assert() { [ "$1" = "0" ] || fail "$2"; }  # $1: 0/1 result of a test, $2: message

run_scenario() {
  local name="$1"; shift
  FAILS=()
  echo ""
  echo "==========================================================="
  echo "> Scenario: $name"
  echo "==========================================================="
  "$@"
  if [ ${#FAILS[@]} -eq 0 ]; then
    echo "PASS  $name"
    REPORT+=("PASS  $name")
    PASS_COUNT=$((PASS_COUNT + 1))
  else
    echo "FAIL  $name"
    for f in "${FAILS[@]}"; do echo "      * $f"; done
    REPORT+=("FAIL  $name")
    for f in "${FAILS[@]}"; do REPORT+=("        * $f"); done
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
}

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/ic-e2e-XXXXXX")"
echo "e2e work dir: $WORKDIR"
cd "$WORKDIR"

# --- Scenario A: basic passthrough -----------------------------------

scenario_a() {
  write_config "e2e|Sink|custom|rtmp://127.0.0.1:1936/live|stream"
  local sink ic
  sink=$(start_sink 1936 A.sink.log)
  ic=$(start_ic A.ic.log)
  wait_port 1936 15; assert $? "sink never opened :1936"
  wait_port 1935 15; assert $? "instantclone never opened :1935"
  if [ ${#FAILS[@]} -eq 0 ]; then
    push_ffmpeg 6
    sleep 3
    grep -q "publish accepted" A.sink.log; assert $? "sink never accepted publish"
    grep -Eq "[1-9][0-9]* IDR" A.sink.log; assert $? "sink saw zero IDRs"
    grep -Eq "audio=[1-9][0-9]*" A.sink.log; assert $? "sink saw zero audio frames"
  fi
  stop_safe "$ic"; stop_safe "$sink"; sleep 1
}

# --- Scenario B: publisher reconnect ---------------------------------

scenario_b() {
  write_config "e2e|Sink|custom|rtmp://127.0.0.1:1936/live|stream"
  local sink ic accepts
  sink=$(start_sink 1936 B.sink.log)
  ic=$(start_ic B.ic.log)
  wait_port 1936 15; assert $? "sink never opened :1936"
  wait_port 1935 15; assert $? "instantclone never opened :1935"
  if [ ${#FAILS[@]} -eq 0 ]; then
    push_ffmpeg 3          # session 1
    sleep 2
    sleep 2               # let ingest mark dead + supervisor tick
    push_ffmpeg 3          # session 2 - must reach the sink cleanly
    sleep 3
    accepts=$(grep -c "publish accepted" B.sink.log)
    assert "$([ "$accepts" -ge 2 ] && echo 0 || echo 1)" "sink only saw $accepts publish accepts (expected >= 2)"
    grep -Eq "[1-9][0-9]* IDR" B.sink.log; assert $? "sink saw zero IDRs across the two sessions"
  fi
  stop_safe "$ic"; stop_safe "$sink"; sleep 1
}

# --- Scenario C: multi-destination ------------------------------------

scenario_c() {
  write_config \
    "d1|SinkOne|custom|rtmp://127.0.0.1:1936/live|s1" \
    "d2|SinkTwo|custom|rtmp://127.0.0.1:1937/live|s2"
  local sink1 sink2 ic
  sink1=$(start_sink 1936 C.sink1.log)
  sink2=$(start_sink 1937 C.sink2.log)
  ic=$(start_ic C.ic.log)
  wait_port 1936 15; assert $? "sink1 never opened :1936"
  wait_port 1937 15; assert $? "sink2 never opened :1937"
  wait_port 1935 15; assert $? "instantclone never opened :1935"
  if [ ${#FAILS[@]} -eq 0 ]; then
    push_ffmpeg 6
    sleep 3
    grep -q "publish accepted" C.sink1.log; assert $? "sink1 never accepted publish"
    grep -q "publish accepted" C.sink2.log; assert $? "sink2 never accepted publish"
    grep -Eq "[1-9][0-9]* IDR" C.sink1.log; assert $? "sink1 saw zero IDRs"
    grep -Eq "[1-9][0-9]* IDR" C.sink2.log; assert $? "sink2 saw zero IDRs"
  fi
  stop_safe "$ic"; stop_safe "$sink1"; stop_safe "$sink2"; sleep 1
}

# --- Scenario D: HTTP API smoke ---------------------------------------

scenario_d() {
  write_config "e2e|Sink|custom|rtmp://127.0.0.1:1936/live|stream"
  local ic cfg
  ic=$(start_ic D.ic.log)
  wait_http "http://127.0.0.1:7799/state" 15; assert $? "web UI never came up on :7799"
  if [ ${#FAILS[@]} -eq 0 ]; then
    local s; s=$(get_state)
    for k in phase ingest_alive destinations stats; do
      [ "$(echo "$s" | jq "has(\"$k\")")" = "true" ]; assert $? "GET /state missing key '$k'"
    done

    curl -s -X POST --data "ms=2000" -H "Content-Type: application/x-www-form-urlencoded" http://127.0.0.1:7799/arm >/dev/null
    sleep 0.3
    [ "$(get_state | jq -r .phase)" != "idle" ]; assert $? "phase still 'idle' after /arm ms=2000"

    curl -s -X POST http://127.0.0.1:7799/disarm >/dev/null
    sleep 0.3
    [ "$(get_state | jq -r .phase)" = "idle" ]; assert $? "phase not 'idle' after /disarm"

    curl -s -X POST "http://127.0.0.1:7799/config/reset?scope=settings" >/dev/null
    sleep 0.3
    cfg=$(curl -s http://127.0.0.1:7799/config)
    [ "$(echo "$cfg" | jq -r .ingest_port)" = "1935" ]; assert $? "ingest_port not reset to default"
    [ "$(echo "$cfg" | jq -r .web_port)" = "7799" ]; assert $? "web_port not reset to default"
    [ "$(echo "$cfg" | jq '.destinations | length')" -ge 1 ]; assert $? "destinations wiped by scope=settings reset"

    curl -s -X POST "http://127.0.0.1:7799/config/reset?scope=all" >/dev/null
    sleep 0.3
    cfg=$(curl -s http://127.0.0.1:7799/config)
    [ "$(echo "$cfg" | jq '.destinations | length')" -eq 0 ]; assert $? "destinations not wiped by scope=all reset"
    [ "$(echo "$cfg" | jq -r .configured)" = "false" ]; assert $? "configured not flipped to false by scope=all reset"
  fi
  stop_safe "$ic"; sleep 1
}

# --- Scenario E: delay + IDR-aligned cut ------------------------------

scenario_e() {
  write_config "e2e|Sink|custom|rtmp://127.0.0.1:1936/live|stream"
  local sink ic ff before after accepts
  sink=$(start_sink 1936 E.sink.log)
  ic=$(start_ic E.ic.log)
  wait_port 1936 15; assert $? "sink never opened :1936"
  wait_port 1935 15; assert $? "instantclone never opened :1935"
  wait_http "http://127.0.0.1:7799/state" 15; assert $? "web UI never came up on :7799"
  if [ ${#FAILS[@]} -eq 0 ]; then
    ff=$(start_ffmpeg 18)
    wait_state '.ingest_alive == true' 15; assert $? "ingest never went live"
  fi
  if [ ${#FAILS[@]} -eq 0 ]; then
    curl -s -X POST --data "ms=2000" -H "Content-Type: application/x-www-form-urlencoded" http://127.0.0.1:7799/arm >/dev/null
    wait_state '.phase == "ready"' 15; assert $? "phase never reached 'ready' after arming 2 s (ring did not fill)"
  fi
  if [ ${#FAILS[@]} -eq 0 ]; then
    sleep 0.5
    before=$(grep -oE '\([0-9]+ IDR\)' E.sink.log | wc -l)
    curl -s -X POST http://127.0.0.1:7799/activate >/dev/null
    wait_state '.phase=="active" and .current_delay_ms>0' 10; assert $? "delay never engaged after /activate"
    sleep 4
    after=$(grep -oE '\([0-9]+ IDR\)' E.sink.log | wc -l)
    accepts=$(grep -c "publish accepted" E.sink.log)
    assert "$([ "$accepts" -eq 1 ] && echo 0 || echo 1)" "sink saw $accepts publish accepts (expected exactly 1 - the cut broke the downstream)"
    assert "$([ "$after" -gt "$before" ] && echo 0 || echo 1)" "sink stopped receiving after the cut ($before -> $after windows)"
    grep -Eq '\([1-9][0-9]* IDR\)' E.sink.log; assert $? "sink saw no IDR windows across the delayed stream"
  fi
  stop_safe "$ff"; stop_safe "$ic"; stop_safe "$sink"; sleep 1
}

# --- Scenario F: scheduled cut ("cut after this airs") ----------------

scenario_f() {
  write_config "e2e|Sink|custom|rtmp://127.0.0.1:1936/live|stream"
  local sink ic ff code r s accepts
  sink=$(start_sink 1936 F.sink.log)
  ic=$(start_ic F.ic.log)
  wait_port 1936 15; assert $? "sink never opened :1936"
  wait_port 1935 15; assert $? "instantclone never opened :1935"
  wait_http "http://127.0.0.1:7799/state" 15; assert $? "web UI never came up on :7799"
  if [ ${#FAILS[@]} -eq 0 ]; then
    code=$(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:7799/cut-after)
    assert "$([ "$code" = "409" ] && echo 0 || echo 1)" "POST /cut-after with no active delay returned $code (want 409)"

    ff=$(start_ffmpeg 25)
    wait_state '.ingest_alive == true' 15; assert $? "ingest never went live"
  fi
  if [ ${#FAILS[@]} -eq 0 ]; then
    curl -s -X POST --data "ms=2000" -H "Content-Type: application/x-www-form-urlencoded" http://127.0.0.1:7799/arm >/dev/null
    wait_state '.phase == "ready"' 15; assert $? "phase never reached 'ready' after arming 2 s"
    curl -s -X POST http://127.0.0.1:7799/activate >/dev/null
    wait_state '.phase=="active" and .current_delay_ms>0' 10; assert $? "delay never engaged after /activate"
  fi
  if [ ${#FAILS[@]} -eq 0 ]; then
    # Schedule then cancel: the mark must drop WITHOUT cutting.
    r=$(curl -s -X POST http://127.0.0.1:7799/cut-after)
    [ "$(echo "$r" | jq -r .safe_cut_pending)" = "true" ]; assert $? "/cut-after did not set safe_cut_pending"
    [ "$(echo "$r" | jq -r .safe_cut_remaining_ms)" -gt 0 ] 2>/dev/null; assert $? "/cut-after reported a zero countdown"
    curl -s -X POST http://127.0.0.1:7799/cut-after/cancel >/dev/null
    s=$(get_state)
    [ "$(echo "$s" | jq -r .safe_cut_pending)" = "false" ]; assert $? "cancel did not clear the pending mark"
    [ "$(echo "$s" | jq -r .phase)" = "active" ]; assert $? "cancel cut the delay (want 'active')"

    # Re-schedule and let it fire: proxy must auto-cut back to passthrough.
    curl -s -X POST http://127.0.0.1:7799/cut-after >/dev/null
    wait_state '.safe_cut_pending==false and .phase=="ready"' 12; assert $? "scheduled cut never fired (want pending=false + phase 'ready')"
    sleep 2
    accepts=$(grep -c "publish accepted" F.sink.log)
    assert "$([ "$accepts" -eq 1 ] && echo 0 || echo 1)" "sink saw $accepts publish accepts (expected exactly 1 - the scheduled cut broke the downstream)"
  fi
  stop_safe "$ff"; stop_safe "$ic"; stop_safe "$sink"; sleep 1
}

# --- Run --------------------------------------------------------------

run_scenario "A -basic passthrough (1 sink, 1 publisher)" scenario_a
run_scenario "B -publisher reconnect (catches seq-header leak)" scenario_b
run_scenario "C -multi-destination (1 publisher, 2 sinks)" scenario_c
run_scenario "D -HTTP API smoke (arm/state/disarm/reset)" scenario_d
run_scenario "E -delay + IDR-aligned cut (arm/ready/activate)" scenario_e
run_scenario "F -scheduled cut (/cut-after fires once the mark airs)" scenario_f

# --- Report -----------------------------------------------------------

echo ""
echo "==========================================================="
echo "  E2E REPORT"
echo "==========================================================="
for line in "${REPORT[@]}"; do echo "  $line"; done
echo ""
if [ "$FAIL_COUNT" -gt 0 ]; then
  echo "e2e: $FAIL_COUNT of $((PASS_COUNT + FAIL_COUNT)) scenarios failed"
  exit 1
fi
echo "e2e: all $PASS_COUNT scenarios passed"
