#!/usr/bin/env bash
# Spawn a fresh Terminal.app window, run the walkthrough there in fullscreen,
# screencapture just that window's pixels. The recording shows ONLY the demo —
# no Claude Code chrome, no editor, no whatever else is on your screen.
#
# Output: demo/recordings/walkthrough-clean-<timestamp>.mov
#
# Requires:
#   - macOS Terminal.app (default)
#   - Screen Recording permission for whatever spawned this script
#   - osascript (built-in)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REC_DIR="$REPO_ROOT/demo/recordings"
mkdir -p "$REC_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$REC_DIR/walkthrough-clean-$STAMP.mov"
DURATION="${REC_DURATION:-360}"
DEMO_PAUSE="${DEMO_PAUSE:-1.5}"

if [[ ! -x "$REPO_ROOT/target/release/tracemind" ]]; then
  echo "Build first: cargo build --release --workspace --exclude tm-tauri" >&2
  exit 1
fi

# Sentinel file so we can poll for completion from outside the Terminal window.
DONE="/tmp/tm-walkthrough-done.$$"
rm -f "$DONE"

# Detect the main display's logical-point dimensions so the Terminal window
# fits the screen (and the recording rect matches the window).
read SCREEN_W SCREEN_H < <(osascript -e 'tell application "Finder" to get bounds of window of desktop' \
    | awk -F', ' '{print $3, $4}')
SCREEN_W="${SCREEN_W:-1440}"
SCREEN_H="${SCREEN_H:-900}"
# Inset by 20pt on each side so the window has a visible border (looks intentional).
WIN_X=20
WIN_Y=40
WIN_W=$(( SCREEN_W - 40 ))
WIN_H=$(( SCREEN_H - 60 ))
WIN_RIGHT=$(( WIN_X + WIN_W ))
WIN_BOTTOM=$(( WIN_Y + WIN_H ))

echo "Detected screen: ${SCREEN_W}x${SCREEN_H}pt → window ${WIN_W}x${WIN_H} at (${WIN_X},${WIN_Y})"

# 1. Spawn a new Terminal window running the walkthrough, sized to fit the screen.
#    The trailing `touch $DONE` lets us detect completion. The window stays open
#    a moment after the walkthrough so the closing frame gets captured.
osascript <<APPLESCRIPT
tell application "Terminal"
    activate
    set newTab to do script "cd '$REPO_ROOT' && clear && DEMO_PAUSE=$DEMO_PAUSE ./demo/walkthrough.sh; sleep 4; touch '$DONE'; exit"
    delay 0.4
    set bounds of front window to {$WIN_X, $WIN_Y, $WIN_RIGHT, $WIN_BOTTOM}
end tell
APPLESCRIPT

# 2. Give the spawned Terminal a moment to settle and come to front.
sleep 1.2

# 3. Bring the new Terminal window to front explicitly (in case anything else
#    grabbed focus) and start the recording.
osascript -e 'tell application "Terminal" to activate' >/dev/null 2>&1 || true
sleep 0.3

echo "Recording → $OUT"
echo "Walkthrough is running in the new Terminal window. Don't click on Claude Code."
screencapture -v -V "$DURATION" -R "${WIN_X},${WIN_Y},${WIN_W},${WIN_H}" -x "$OUT" &
REC_PID=$!

# 4. Wait for the walkthrough to signal completion (sentinel file).
deadline=$(( $(date +%s) + DURATION - 5 ))
while [[ ! -f "$DONE" ]]; do
  if [[ $(date +%s) -ge $deadline ]]; then
    echo "(walkthrough timeout — stopping recording)"
    break
  fi
  sleep 1
done

# 5. Final beat for closing frame, then stop the recording cleanly.
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
