#!/usr/bin/env bash
# ------------------------------------------------------------------
# TraceMind ambient-capture demo (2026-07-25).
#
# Golden-path screen recording. Shows the story that shipped this
# week: browser-history ingest, honest Safari permission ask, URL
# extraction that lands the site's organization as a graph node,
# quick-recall, and the weekly retention digest.
#
# Usage
#   PAUSE_SHOT=5 PAUSE_RUN=2.5 PAUSE_READ=4.5 scripts/demo_ambient.sh
#   PAUSE_SHOT=0 PAUSE_RUN=0   PAUSE_READ=0   scripts/demo_ambient.sh   # CI
#
# Env
#   BIN         — tracemind binary (default: ./target/release/tracemind)
#   TM_DATA_DIR — data dir override (default: /tmp/tm-demo-ambient)
#   PAUSE_SHOT  — seconds on a shot heading (default 5)
#   PAUSE_RUN   — seconds around each command (default 2.5)
#   PAUSE_READ  — seconds to leave a block on screen (default 4.5)
#
# All ingests use real BGE embeddings (no --hash-embed). No network
# calls; everything runs on-device.
# ------------------------------------------------------------------

set -euo pipefail

BIN="${BIN:-./target/release/tracemind}"
DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-ambient}"
PAUSE_SHOT="${PAUSE_SHOT:-5}"
PAUSE_RUN="${PAUSE_RUN:-2.5}"
PAUSE_READ="${PAUSE_READ:-4.5}"

export TM_DATA_DIR="$DATA_DIR"

if [ ! -x "$BIN" ]; then
    echo "==> building release binary (one-time)..."
    cargo build --release -p tm-cli >/dev/null 2>&1
fi

# Pretty formatting helpers.
BOLD=$'\033[1m'; DIM=$'\033[2m'; CYAN=$'\033[36m'; YEL=$'\033[33m'; RESET=$'\033[0m'

shot() {
    printf '\n%s\n' "${BOLD}${CYAN}▸ $1${RESET}"
    printf '%s\n' "${DIM}$2${RESET}"
    sleep "$PAUSE_SHOT"
}

run() {
    printf '\n%s%s%s\n' "${YEL}\$ " "$1" "${RESET}"
    sleep "$PAUSE_RUN"
    # shellcheck disable=SC2086
    eval "$1"
    sleep "$PAUSE_RUN"
}

read_beat() { sleep "$PAUSE_READ"; }

# ------------------------------------------------------------------
# 0. Fresh state so the demo is reproducible.
# ------------------------------------------------------------------
rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

shot "Cold start" \
     "TraceMind is a local-only memory OS. Everything you're about to see runs on this laptop with no network."

run "$BIN onboard --dry-run"
read_beat

# ------------------------------------------------------------------
# 1. Capture doctor — the honest permission story.
# ------------------------------------------------------------------
shot "Capture doctor" \
     "Ambient capture reads your browser history directly. Chrome works out of the box; Safari's DB is TCC-protected. The doctor tells you exactly what to do — including whether Vision-framework OCR is compiled in for screenshots."

run "$BIN capture-doctor"
read_beat

# ------------------------------------------------------------------
# 1b. Screenshot OCR (only when the build has it).
# ------------------------------------------------------------------
LATEST_SHOT=""
if [ -d "$HOME/Desktop" ]; then
    LATEST_SHOT=$(ls -t "$HOME"/Desktop/Screenshot*.png 2>/dev/null | head -1 || true)
fi
if [ -n "$LATEST_SHOT" ] && "$BIN" capture-doctor 2>/dev/null | grep -q '\[  OK\] screenshot-ocr'; then
    shot "Screenshot OCR (macOS Vision)" \
         "The screenshot watcher polls ~/Desktop for PNGs; when built with --features macos-screencapture, Vision-framework OCR fills in payload.text so retrieval works over the *contents* of every shot."

    run "$BIN capture-doctor --ocr '$LATEST_SHOT' | head -20"
    read_beat
fi

# ------------------------------------------------------------------
# 2. Simulated ambient browsing — a batch of realistic visits.
# ------------------------------------------------------------------
shot "Ambient browsing (simulated)" \
     "Instead of driving Chrome live, we ingest a batch of realistic visits — same shape the daemon emits from the real history DB, so what you see mirrors production."

VISITS=(
    "https://www.fifa.com/tickets FIFA World Cup 2026 Tickets"
    "https://www.fifa.com/en/tournaments/mens/worldcup FIFA World Cup — Official Site"
    "https://en.wikipedia.org/wiki/2026_FIFA_World_Cup 2026 FIFA World Cup — Wikipedia"
    "https://apnews.com/article/world-cup-2026-tickets Ticket resale prices for the 2026 World Cup"
    "https://arxiv.org/abs/2312.10997 GEPA: Reflective Prompt Evolution"
    "https://arxiv.org/abs/2403.02419 ColBERT-XM: multilingual retrieval"
    "https://superlinked.com/vectorhub Vector Hub — Superlinked"
)

for v in "${VISITS[@]}"; do
    printf '\n%s%s%s\n' "${YEL}\$ tracemind ingest \"$v\"${RESET}"
    sleep 0.6
    "$BIN" ingest "$v" 2>/dev/null | grep -E '^\s*(Entities|\[|\+|Ingested|Trace)' | head -8
done
read_beat

# ------------------------------------------------------------------
# 3. Entities landed as graph nodes.
# ------------------------------------------------------------------
shot "The graph now knows the sites" \
     "Notice each capture landed a raw URL entity AND the site's organization (fifa.com, wikipedia.org, arxiv.org). A query for 'FIFA' can now hit the graph directly, not just BGE similarity."

run "$BIN query 'fifa world cup tickets'"
read_beat

# ------------------------------------------------------------------
# 4. Quick recall — the launcher target.
# ------------------------------------------------------------------
shot "Quick recall (menu-bar launcher target)" \
     "Bind this to a global shortcut in Raycast/Alfred: one keystroke → the site you're thinking of. Small top_k, no LLM, sub-100ms."

run "$BIN quick-recall 'world cup' --top-k 5"
read_beat
run "$BIN quick-recall 'reflective prompt' --top-k 3"
read_beat

# ------------------------------------------------------------------
# 5. Weekly retention digest.
# ------------------------------------------------------------------
shot "Weekly retention digest" \
     "Every week the daemon summarizes what you captured, unresolved contradictions, and open commitments. Written to ~/.tracemind/notifications/ so a menu-bar app can badge it."

run "$BIN digest --persist --recent-days 7"
read_beat

# ------------------------------------------------------------------
# 6. Wrap.
# ------------------------------------------------------------------
shot "Wrap" \
     "Every byte you saw stayed on this laptop. No API keys, no cloud, no telemetry. Ambient browser capture, honest permission story, first-class graph nodes for the sites you visit, sub-100ms recall."
