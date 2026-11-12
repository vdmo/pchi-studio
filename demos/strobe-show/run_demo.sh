#!/usr/bin/env bash
# Strobe x PCHI demo — Shared Show State in Mission Control.
#
# Real: the Conductor, the Only-Lang rule + signed governance log, the real
# strobe-pchi-bridge, the show-state model and the /state endpoint.
# Simulated: Strobe itself (fake_strobe.py speaks Strobe's documented OSC
# addresses) and the lights/lasers/audio departments (pchi_show.py dept).
#
# Usage:  ./run_demo.sh            run the scripted demo and check the result
#         ./run_demo.sh --stay     same, then keep everything running so you
#                                  can watch http://127.0.0.1:3000/mission-control
set -euo pipefail

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$DEMO_DIR/../.." && pwd)"
SCRATCH="$(mktemp -d)"
LOG_DIR="$SCRATCH/logs"
VENV="${PCHI_DEMO_VENV:-$HOME/.cache/pchi-demo-venv}"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
STAY=0; [[ "${1:-}" == "--stay" ]] && STAY=1
mkdir -p "$LOG_DIR"

PIDS=()
cleanup() {
  echo; echo "--- shutting down ---"
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

if [[ ! -x "$VENV/bin/python" ]]; then
  echo "--- creating venv $VENV (python-osc, websocket-client) ---"
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install --quiet python-osc websocket-client
fi
PY="$VENV/bin/python"

echo "--- building the Conductor ---"
(cd "$REPO_ROOT" && cargo build -p primeswarm-pchi --quiet)

echo "--- starting the Conductor with the demo rule ---"
(cd "$SCRATCH" && exec "$TARGET_DIR/debug/primeswarm-pchi" --rules "$DEMO_DIR/strobe-show-rules.only-pchi") \
  > "$LOG_DIR/conductor.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 40); do
  curl -sf http://127.0.0.1:3000/state >/dev/null 2>&1 && break
  sleep 0.25
done
curl -sf http://127.0.0.1:3000/state >/dev/null || { echo "Conductor did not come up; see $LOG_DIR/conductor.log"; exit 1; }

snapshot() { curl -sf http://127.0.0.1:3000/state; }
field() { "$PY" -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

echo "--- starting the Strobe bridge ---"
"$PY" -u "$REPO_ROOT/tools/strobe-pchi-bridge/strobe_pchi_bridge.py" --config "$DEMO_DIR/strobe_show.json" \
  > "$LOG_DIR/bridge.log" 2>&1 &
PIDS+=($!)
sleep 1

echo "--- starting simulated departments (lights, audio, lasers) ---"
"$PY" -u "$REPO_ROOT/tools/pchi-show/pchi_show.py" dept lights --source sim_lights > "$LOG_DIR/lights.log" 2>&1 &
PIDS+=($!)
"$PY" -u "$REPO_ROOT/tools/pchi-show/pchi_show.py" dept audio --source sim_audio > "$LOG_DIR/audio.log" 2>&1 &
PIDS+=($!)
"$PY" -u "$REPO_ROOT/tools/pchi-show/pchi_show.py" dept lasers --source sim_lasers \
  --schedule "0:not_ready:safety check pending,10:ready" > "$LOG_DIR/lasers.log" 2>&1 &
PIDS+=($!)

echo "--- starting fake Strobe (playlist advances every 12 s; an operator ramps strobe_intensity from t=5 s) ---"
"$PY" -u "$REPO_ROOT/tools/strobe-pchi-bridge/fake_strobe.py" --auto-advance 12 --ramp-strobe-after 5 \
  > "$LOG_DIR/strobe.log" 2>&1 &
PIDS+=($!)

echo
echo "Mission Control: http://127.0.0.1:3000/mission-control   (logs: $LOG_DIR)"
sleep 3

echo
echo "=== t=3s: Strobe is on cue 1; lasers are not ready ==="
snapshot > "$SCRATCH/early.json"
echo "current:   $(field "d['current']" < "$SCRATCH/early.json")"
echo "confidence:$(field "d['confidence']" < "$SCRATCH/early.json")"
echo "at risk:   $(field "d['blockers']" < "$SCRATCH/early.json")"

echo
echo "=== gate 1: an operator asks for FINALE while lasers are not ready -> refused ==="
"$PY" "$REPO_ROOT/tools/pchi-show/pchi_show.py" enter FINALE --source operator
sleep 1
snapshot > "$SCRATCH/gated.json"
echo "current:   $(field "d['current']" < "$SCRATCH/gated.json")   (should still be PRESHOW)"

echo
echo "=== gate 2: Strobe itself jumps to cue 5 (FINALE) -> refused, bridge returns Strobe to cue 1 ==="
"$PY" -c "from pythonosc.udp_client import SimpleUDPClient as C; C('127.0.0.1', 9000).send_message('/strobe/cue/5', 1)"
sleep 2
snapshot > "$SCRATCH/gated2.json"
echo "current:   $(field "d['current']" < "$SCRATCH/gated2.json")   (should still be PRESHOW)"
grep -E "cue -> (5|1)$" "$LOG_DIR/strobe.log" || true

echo
echo "=== operator enters BUILD from outside Strobe (PCHI -> bridge -> Strobe cue 3) ==="
"$PY" "$REPO_ROOT/tools/pchi-show/pchi_show.py" enter BUILD --source operator
sleep 2
snapshot > "$SCRATCH/driven.json"
echo "current:   $(field "d['current']" < "$SCRATCH/driven.json")"
grep -m1 "cue -> 3" "$LOG_DIR/strobe.log" || true

echo
echo "--- waiting for lasers to become ready and the strobe ramp to cross the rule (t~20s) ---"
sleep 15
snapshot > "$SCRATCH/late.json"
echo
echo "=== t~20s ==="
field "'current:    %s' % d['current']" < "$SCRATCH/late.json"
field "'next:       %s' % d['next']" < "$SCRATCH/late.json"
field "'confidence: %s' % d['confidence']" < "$SCRATCH/late.json"
field "'denials:    %s' % [x['message'] for x in d['recent_denials']]" < "$SCRATCH/late.json"
field "'departments:%s' % {x['name']: x['status'] for x in d['departments']}" < "$SCRATCH/late.json"

echo
echo "=== checks ==="
FAIL=0
check() { # description, python expression over d (file)
  if "$PY" -c "import json,sys; d=json.load(open('$3')); sys.exit(0 if ($2) else 1)"; then
    echo "PASS  $1"
  else
    echo "FAIL  $1"; FAIL=1
  fi
}
check "Strobe's cue became the shared current state"          "d['current'] == 'PRESHOW'" "$SCRATCH/early.json"
check "lasers not ready is visible as an at-risk blocker"      "any('lasers' in b for b in d['blockers'])" "$SCRATCH/early.json"
check "confidence is below 100 while lasers are not ready"     "d['confidence'] is not None and d['confidence']['score'] < 100" "$SCRATCH/early.json"
check "an operator's enter drove Strobe (PCHI -> Strobe)"      "d['current'] == 'BUILD'" "$SCRATCH/driven.json"

grep -q "cue -> 3" "$LOG_DIR/strobe.log" && echo "PASS  fake Strobe received /strobe/cue/3" || { echo "FAIL  fake Strobe never received /strobe/cue/3"; FAIL=1; }
check "gate 1: FINALE was refused while lasers were not ready" "d['current'] == 'PRESHOW' and any('FINALE' in x['message'] for x in d['recent_denials'])" "$SCRATCH/gated.json"
check "gate 2: the show did not follow Strobe into FINALE" "d['current'] == 'PRESHOW'" "$SCRATCH/gated2.json"
"$PY" - "$LOG_DIR/strobe.log" <<'EOF' && echo "PASS  gate 2: Strobe went to cue 5, then the bridge returned it to cue 1" || { echo "FAIL  gate 2: Strobe was not returned to cue 1"; FAIL=1; }
import re, sys
cues = re.findall(r"cue -> (\d+)", open(sys.argv[1]).read())
sys.exit(0 if cues[:2] == ["5", "1"] else 1)
EOF
check "four departments are reporting"                         "len(d['departments']) == 4" "$SCRATCH/late.json"
check "the strobe rule denied the ramp and it lowered confidence" "len(d['recent_denials']) >= 1 and d['confidence']['denial_penalty_pct'] > 0" "$SCRATCH/late.json"

if [[ "$STAY" == 1 ]]; then
  echo; echo "Leaving everything running. Open http://127.0.0.1:3000/mission-control — Ctrl+C to stop."
  while true; do sleep 3600; done
fi
exit "$FAIL"
