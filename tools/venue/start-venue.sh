#!/usr/bin/env bash
# Start PCHI Studio's runtime on the venue computer, and (if Tailscale and Caddy are there) make Studio reachable
# from other machines on your private Tailscale network. See docs/remote-setup.md.
#
#   bash tools/venue/start-venue.sh
#
# Settings go in tools/venue/venue.conf (copy venue.conf.example) or the environment. Defaults are safe:
#   - the runtime listens on 127.0.0.1 only: the only way in from outside is the web door below
#   - cues are NOT sent to the rig until you set SEND=1 (a dry run says what would be sent)
#   - it refuses to start without a roster that has an operator
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
# shellcheck disable=SC1091
[ -f "${VENUE_CONF:-$here/venue.conf}" ] && . "${VENUE_CONF:-$here/venue.conf}"

DATA="${DATA:-$HOME/pchi-venue}"          # everything the runtime keeps: roster, library, memory, receipts, log
BIN="${BIN:-pchi-studio-api}"             # the runtime binary (or a path such as /opt/pchi/pchi-studio-api)
SHOW="${SHOW:-}"                          # the show file to start with (.pchi)
PROFILES="${PROFILES:-}"                  # extra device profiles folder, e.g. tools/tixl/profiles
PORT="${PORT:-3100}"                      # the runtime, on loopback
WEB_PORT="${WEB_PORT:-8080}"              # the web door (Studio + /api), on the Tailscale address only
STUDIO_DIST="${STUDIO_DIST:-$repo/pchi-studio-react/dist}"   # a built Studio (npm run build)
SEND="${SEND:-0}"                         # 1 = really send cues to the rig
TS_IP="${TS_IP:-}"                        # override the Tailscale address (normally found by itself)

die() { echo "start-venue: $*" >&2; exit 1; }

[ -n "$SHOW" ] && [ -f "$SHOW" ] || die "set SHOW to the show file to start with (a .pchi), in venue.conf or the environment"
command -v "$BIN" >/dev/null 2>&1 || [ -x "$BIN" ] || die "cannot find the runtime '$BIN': set BIN to its path"
[ -f "$DATA/roster.json" ] || die "no roster yet: run  bash tools/venue/add-person.sh YourName operator  first"
grep -Eq '"role"[[:space:]]*:[[:space:]]*"operator"' "$DATA/roster.json" || die "the roster has no operator: add one with add-person.sh"

umask 077
mkdir -p "$DATA/library" "$DATA/governance" "$DATA/secret" "$DATA/logs"

args=("$SHOW"
  --bind "127.0.0.1:$PORT"
  --roster "$DATA/roster.json"
  --library "$DATA/library"
  --memory "$DATA/memory.db"
  --db "$DATA/show.db"
  --record "$DATA/logs/run-$(date +%Y%m%d-%H%M%S).jsonl"
  --governance "$DATA/governance"
  --governance-key "$DATA/secret/governance.key"
  --sessions "$DATA/sessions.json")
[ -s "$DATA/show.db" ] && args+=(--resume)          # carry on the run that was going if the computer restarted
[ -n "$PROFILES" ] && args+=(--profiles "$PROFILES")
[ "$SEND" = "1" ] && args+=(--send)

pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM

echo "runtime: $BIN on 127.0.0.1:$PORT  ($([ "$SEND" = 1 ] && echo 'SENDING cues to the rig' || echo 'dry run: set SEND=1 to send cues'))"
"$BIN" "${args[@]}" &
pids+=($!)

# wait until it answers
for _ in $(seq 1 50); do
  curl -s -o /dev/null "http://127.0.0.1:$PORT/state" && break
  kill -0 "${pids[0]}" 2>/dev/null || die "the runtime stopped at startup (see its message above)"
  sleep 0.2
done

# ---- the web door: Studio and /api on the Tailscale address only ----
if [ -z "$TS_IP" ] && command -v tailscale >/dev/null 2>&1; then
  TS_IP="$(tailscale ip -4 2>/dev/null | head -n1 || true)"
fi
if [ -z "$TS_IP" ]; then
  echo "remote: no Tailscale address, so the runtime is reachable on this machine only (127.0.0.1:$PORT). See docs/remote-setup.md."
elif [ ! -f "$STUDIO_DIST/index.html" ]; then
  echo "remote: Studio is not built ($STUDIO_DIST/index.html is missing): cd pchi-studio-react && npm ci && npm run build"
else
  cat > "$DATA/Caddyfile" <<EOF
{
	admin off
	auto_https off
}
http://$TS_IP:$WEB_PORT {
	bind $TS_IP
	# the runtime: /api/x on the web is /x on the runtime (the live feed's WebSocket too)
	handle_path /api/* {
		reverse_proxy 127.0.0.1:$PORT
	}
	# Studio: any other address falls back to the app
	handle {
		root * $STUDIO_DIST
		try_files {path} /index.html
		file_server
	}
}
EOF
  if command -v caddy >/dev/null 2>&1; then
    caddy run --config "$DATA/Caddyfile" --adapter caddyfile &
    pids+=($!)
    echo "remote: Studio is at http://$TS_IP:$WEB_PORT/ for people on your Tailscale network"
  else
    echo "remote: Caddy is not installed (https://caddyserver.com/docs/install). Its config is ready at $DATA/Caddyfile"
  fi
fi

echo "running: Ctrl+C stops everything."
wait -n "${pids[@]}" || true
echo "start-venue: something stopped; shutting down the rest."
