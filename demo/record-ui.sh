#!/usr/bin/env bash
# Record the UI walkthrough as a full-screen .mov.
#
# The presenter runs ./demo/ui-walkthrough.sh in a terminal (narration cards
# print there). The Tauri window appears on the desktop. This wrapper:
#   1. Starts a screencapture of the entire main display
#   2. Kicks off ui-walkthrough.sh, which seeds data and launches tracemind-app
#   3. Waits for the presenter to finish clicking through (or REC_DURATION cap)
#   4. Stops the recording cleanly
#
# Output: demo/recordings/ui-walkthrough-<timestamp>.mov
#
# Requires Screen Recording permission for whoever spawns this script.
#
# Manual alternative: Cmd+Shift+5 → Record Entire Screen, then run
#   ./demo/ui-walkthrough.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REC_DIR="$REPO_ROOT/demo/recordings"
mkdir -p "$REC_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$REC_DIR/ui-walkthrough-$STAMP.mov"
DURATION="${REC_DURATION:-300}"   # 5 min cap
DEMO_PAUSE="${DEMO_PAUSE:-1.0}"

if [[ ! -x "$REPO_ROOT/target/release/tracemind" ]] || \
   [[ ! -x "$REPO_ROOT/target/release/tracemind-app" ]]; then
  echo "Build first:" >&2
  echo "  cargo build --release -p tm-cli" >&2
  echo "  cargo build --release -p tm-tauri --features custom-protocol" >&2
  exit 1
fi

echo "Recording → $OUT (cap: ${DURATION}s)"
echo "Walkthrough cues will print in this terminal. Tauri app opens in its own window."
echo "Press Ctrl-C in this terminal when you're done; recording will be finalised."
sleep 2

# Full-display capture — the Tauri window can move freely.
screencapture -v -V "$DURATION" -x "$OUT" &
REC_PID=$!

# Give screencapture a moment to start before we open the app.
sleep 1

# Kick off the walkthrough (foreground so the cues land in this terminal).
DEMO_PAUSE=$DEMO_PAUSE "$REPO_ROOT/demo/ui-walkthrough.sh" || true

# Give the presenter time to wrap up after the walkthrough script finishes.
# They'll be clicking through the Tauri app at their own pace.
echo
echo "Walkthrough cues done. Recording continues — press Enter to stop, or wait for cap."
read -t "${UI_TRAIL:-60}" -r _ || true

if kill -0 "$REC_PID" 2>/dev/null; then
  kill -INT "$REC_PID" 2>/dev/null || true
  wait "$REC_PID" 2>/dev/null || true
fi

if [[ -f "$OUT" ]]; then
  SIZE=$(du -h "$OUT" | awk '{print $1}')
  echo
  echo "✓ Recording: $OUT  ($SIZE)"
  echo "  Open with: open '$OUT'"
else
  echo "✗ Recording file not produced. Check Screen Recording permission." >&2
  exit 1
fi
