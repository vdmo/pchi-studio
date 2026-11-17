#!/usr/bin/env bash
# Add a person to the venue roster with their own key.
#
#   bash tools/venue/add-person.sh NAME ROLE [DEPARTMENTS]
#   bash tools/venue/add-person.sh Alex operator
#   bash tools/venue/add-person.sh Sam editor lighting          # an editor limited to one department
#
# Roles: operator, editor, lso, guest, agent, button. The key is printed ONCE: give it to the person.
# The roster keeps only a salted hash. The runtime reads the roster at startup, so restart it afterwards.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
[ -f "${VENUE_CONF:-$here/venue.conf}" ] && . "${VENUE_CONF:-$here/venue.conf}"
DATA="${DATA:-$HOME/pchi-venue}"
BIN="${BIN:-pchi-studio-api}"
ROSTER="$DATA/roster.json"

if [ $# -lt 2 ]; then
  echo "usage: add-person.sh NAME ROLE [DEPARTMENTS]   (roles: operator editor lso guest agent button)" >&2
  exit 2
fi
spec="$1:$2${3:+:$3}"

umask 077
mkdir -p "$DATA"

out="$("$BIN" --new-roster-entry "$spec")" || { echo "the runtime refused that entry (see above)" >&2; exit 1; }
key="$(printf '%s\n' "$out" | awk '/^    [0-9a-f]+$/ {print $1; exit}')"
entry="$(printf '%s\n' "$out" | sed -n '/^{/,/^}/p')"
[ -n "$key" ] && [ -n "$entry" ] || { echo "could not read the runtime's answer" >&2; exit 1; }

ENTRY="$entry" ROSTER="$ROSTER" python3 - <<'PY'
import json, os, sys
path, entry = os.environ["ROSTER"], json.loads(os.environ["ENTRY"])
doc = json.load(open(path)) if os.path.exists(path) else {"people": []}
if any(p.get("name") == entry["name"] for p in doc["people"]):
    sys.exit(f"{entry['name']} is already on the roster: pick another name, or edit {path} to remove them first")
doc["people"].append(entry)
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(doc, f, indent=2)
os.chmod(tmp, 0o600)
os.replace(tmp, path)
PY

echo
echo "Added ${1} (${2}). Their key, shown once, give it to them privately:"
echo
echo "    $key"
echo
echo "Restart the runtime (start-venue.sh) so it reads the roster: $ROSTER"
