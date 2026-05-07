#!/usr/bin/env bash
# Record the walkthrough to a .mov file using macOS screencapture.
#
# Requires: Screen Recording permission for whichever process is parent
#   (Terminal / iTerm / VS Code's terminal). Grant in:
#     System Settings → Privacy & Security → Screen & System Audio Recording
#
# Output: demo/recordings/walkthrough-<timestamp>.mov

set -euo pipefail

REC_DIR="demo/recordings"
mkdir -p "$REC_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
OUT="$REC_DIR/walkthrough-$STAMP.mov"
DURATION="${REC_DURATION:-360}"   # 6 min cap

if ! command -v screencapture >/dev/null; then
  echo "screencapture not found (macOS only)." >&2
  exit 1
fi

if [[ ! -x ./target/release/tracemind ]]; then
  echo "Build first: cargo build --release --workspace --exclude tm-tauri" >&2
  exit 1
fi

echo "Recording for up to $DURATION s → $OUT"
echo "Walkthrough starts in 2s. Make sure the terminal window is in front."
sleep 2

# Start recording in background with main display.
# -V <s>  cap duration       -v  video       -x  no sounds
# screencapture writes the file when its duration cap hits OR on SIGINT.
screencapture -v -V "$DURATION" -x "$OUT" &
REC_PID=$!

# Give screencapture a moment to actually start.
sleep 1

# Run the walkthrough.
./demo/walkthrough.sh

# Send SIGINT for graceful flush (NOT plain kill — that drops the file).
if kill -0 "$REC_PID" 2>/dev/null; then
  kill -INT "$REC_PID" 2>/dev/null || true
  wait "$REC_PID" 2>/dev/null || true
fi

# Final sizing.
if [[ -f "$OUT" ]]; then
  SIZE=$(du -h "$OUT" | awk '{print $1}')
  echo
  echo "Recording saved: $OUT  ($SIZE)"
  echo "Open with:       open '$OUT'"
else
  echo "Recording file not produced. Check Screen Recording permission for the parent app."
fi
