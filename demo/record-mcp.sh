#!/usr/bin/env bash
# Record demo/mcp-walkthrough.sh into a clean .mov by spawning a fresh
# Terminal window and screen-capturing just that window's rectangle.
#
# Output: demo/recordings/mcp-walkthrough-<timestamp>.mov
#
# Requires Screen Recording permission for whoever spawns this script
# (Terminal / iTerm / VS Code's terminal / Claude Code).
#
# If you'd rather record manually:
#   1. Cmd+Shift+5 → Record Selected Portion / Entire Screen
#   2. Run ./demo/mcp-walkthrough.sh in your terminal
#   3. Click ⏹ in the menu bar when done

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REC_DIR="$REPO_ROOT/demo/recordings"
mkdir -p "$REC_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$REC_DIR/mcp-walkthrough-$STAMP.mov"
DURATION="${REC_DURATION:-300}"     # 5 min cap
DEMO_PAUSE="${DEMO_PAUSE:-1.0}"

if [[ ! -x "$REPO_ROOT/target/release/tm-mcp" ]]; then
  echo "Build first: cargo build --release -p tm-mcp" >&2
  exit 1
fi

DONE="/tmp/tm-mcp-walkthrough-done.$$"
rm -f "$DONE"

# Compute window rectangle that fills the main display.
read SCREEN_W SCREEN_H < <(osascript -e 'tell application "Finder" to get bounds of window of desktop' \
    | awk -F', ' '{print $3, $4}')
SCREEN_W="${SCREEN_W:-1440}"; SCREEN_H="${SCREEN_H:-900}"
WIN_X=20; WIN_Y=40
WIN_W=$(( SCREEN_W - 40 )); WIN_H=$(( SCREEN_H - 60 ))
WIN_RIGHT=$(( WIN_X + WIN_W )); WIN_BOTTOM=$(( WIN_Y + WIN_H ))

echo "Screen: ${SCREEN_W}x${SCREEN_H}pt → window ${WIN_W}x${WIN_H} at (${WIN_X},${WIN_Y})"

# Spawn fresh Terminal window running the MCP walkthrough.
osascript <<APPLESCRIPT
tell application "Terminal"
    activate
    set newTab to do script "cd '$REPO_ROOT' && clear && DEMO_PAUSE=$DEMO_PAUSE ./demo/mcp-walkthrough.sh; sleep 3; touch '$DONE'; exit"
    delay 0.4
    set bounds of front window to {$WIN_X, $WIN_Y, $WIN_RIGHT, $WIN_BOTTOM}
end tell
APPLESCRIPT

sleep 1.2
osascript -e 'tell application "Terminal" to activate' >/dev/null 2>&1 || true
sleep 0.3

echo "Recording → $OUT"
echo "MCP walkthrough is running in the new Terminal window. Don't click on Claude Code."
screencapture -v -V "$DURATION" -R "${WIN_X},${WIN_Y},${WIN_W},${WIN_H}" -x "$OUT" &
REC_PID=$!

deadline=$(( $(date +%s) + DURATION - 5 ))
while [[ ! -f "$DONE" ]]; do
  if [[ $(date +%s) -ge $deadline ]]; then
    echo "(walkthrough timeout — stopping recording)" >&2
    break
  fi
  sleep 1
done

sleep 2
if kill -0 "$REC_PID" 2>/dev/null; then
  kill -INT "$REC_PID" 2>/dev/null || true
  wait "$REC_PID" 2>/dev/null || true
fi
rm -f "$DONE"

if [[ -f "$OUT" ]]; then
  SIZE=$(du -h "$OUT" | awk '{print $1}')
  echo
  echo "✓ Recording: $OUT  ($SIZE)"
  echo "  Open with: open '$OUT'"
else
  echo "✗ Recording file not produced. Check Screen Recording permission." >&2
  exit 1
fi
