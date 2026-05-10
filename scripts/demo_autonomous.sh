#!/usr/bin/env bash
# Autonomous demo script — exercises every shot from docs/DEMO_SCRIPT.md
# end-to-end via the CLI, no voice / no GUI / no human.
#
# Used to verify the demo flow works after each substantive change. The
# Tauri app would render the same DailyBrief surface this script
# inspects via `tracemind brief --json`, so a clean run here is a
# strong signal the recordable demo is wirable.

set -euo pipefail

DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-autonomous}"
BIN="${BIN:-./target/release/tracemind}"
SEP="────────────────────────────────────────────────────────────────────"
# Optional pacing for screen-recordings; default 0 = run flat-out (CI / verify).
PAUSE_SHOT="${PAUSE_SHOT:-0}"
PAUSE_RUN="${PAUSE_RUN:-0}"

export TM_DATA_DIR="$DATA_DIR"
rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

shot() {
  printf "\n%s\n  SHOT %s\n%s\n" "$SEP" "$1" "$SEP"
  [[ "$PAUSE_SHOT" != 0 ]] && sleep "$PAUSE_SHOT" || true
}

run() {
  printf "\n$ %s\n" "$*"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
  "$@" || { echo "[FAIL] $*"; exit 1; }
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
}

shot "0 — pre-roll: restore deterministic fixture"
run "$BIN" demo restore --force

shot "1 — cold open: brief"
run "$BIN" brief

shot "2a — retraction beat: list contradictions"
run "$BIN" contradictions list

shot "2b — retraction beat: resolve (keep-b → retract A)"
# Pluck the first contradiction's triple pair from the JSON surface.
PAIR=$("$BIN" contradictions list --json | python3 -c '
import json, sys
rows = json.load(sys.stdin)
if not rows:
    sys.stderr.write("no outstanding contradictions to resolve\n")
    sys.exit(2)
r = rows[0]
print(r["triple_a"], r["triple_b"])
')
TRIPLE_A=$(echo "$PAIR" | awk "{print \$1}")
TRIPLE_B=$(echo "$PAIR" | awk "{print \$2}")
echo "resolving pair: $TRIPLE_A  ↔  $TRIPLE_B"
run "$BIN" contradictions resolve "$TRIPLE_A" "$TRIPLE_B" keep-b

shot "2c — retraction beat: verify the row leaves the brief"
run "$BIN" contradictions list

shot "3 — capture beat: ingest a paragraph"
echo "We had a great call with Priya from Mercury yesterday. The contract is moving." \
  | run "$BIN" ingest -

shot "4a — overdue commitment: list open + overdue"
echo "$ $BIN brief --json > /tmp/tm-demo-brief.json"
"$BIN" brief --json 2>/dev/null > /tmp/tm-demo-brief.json
python3 <<'PY'
import json
b = json.load(open("/tmp/tm-demo-brief.json"))
c = b.get("counts", {})
print("  counts: overdue={} open={} resolved={} contradictions={}".format(
    c.get("overdue"), c.get("open"), c.get("resolved"), c.get("contradictions")))
overdue = b.get("overdue", [])
print("  first overdue:", overdue[0]["id"] if overdue else "<none>")
PY

shot "4b — outcome: complete the first overdue with positive polarity"
COMMIT_ID=$(python3 <<'PY'
import json
b = json.load(open("/tmp/tm-demo-brief.json"))
print(b["overdue"][0]["id"] if b.get("overdue") else "")
PY
)
if [[ -n "$COMMIT_ID" ]]; then
  run "$BIN" resolve --polarity better "$COMMIT_ID" "shipped on time"
else
  echo "  (no overdue commitments — fixture has none)"
fi

shot "5 — final brief: contradictions = 0, resolved bumps by one"
run "$BIN" brief

printf "\n%s\n  AUTONOMOUS DEMO COMPLETE — all shots green\n%s\n" "$SEP" "$SEP"
