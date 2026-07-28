#!/usr/bin/env bash
# Record the native desktop app walking every product surface.
#
# Why this exists: the browser preview (`demo-bridge.mjs`) only shims the
# primary-flow handlers, so Composer / Graph / Dashboard / Threads / Ledger /
# Garden / Events / Views / Calibration / Inspector never render there. Those
# surfaces call typed Tauri IPC that the shim has no honest data source for.
# This script drives the real `tracemind-app` instead, so every pixel is the
# real Rust backend.
#
# It needs no Accessibility permission: the app is put into an auto-tour by a
# backend-persisted flag (`$TM_DATA_DIR/demo_tour.json`), goes fullscreen on
# its own, and advances surfaces on a timer. We only take screenshots, which
# needs Screen Recording permission (already granted if `screencapture` works).
#
# Usage:
#   ./scripts/demo_gui_native.sh                 # record at 6s/surface
#   DWELL=4 ./scripts/demo_gui_native.sh         # faster
#   KEEP_DATA=1 ./scripts/demo_gui_native.sh     # don't wipe the data dir

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="$REPO/target/release/tracemind"
APP="$REPO/target/release/tracemind-app"
OUT="$REPO/docs/demos/gui"
DWELL="${DWELL:-6}"
export TM_DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-gui-native}"

BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'; GREEN=$'\033[32m'; RED=$'\033[31m'

say() { printf '%s==>%s %s\n' "$BOLD" "$RESET" "$*"; }
die() { printf '%s!!%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

[ -x "$CLI" ] || die "missing $CLI — run: cargo build --release -p tm-cli"
[ -x "$APP" ] || die "missing $APP — run: PATH=\"\$HOME/Library/Python/3.9/bin:\$PATH\" cargo build --release -p tm-tauri"

mkdir -p "$OUT"

# ── 1. Seed a throwaway memory ────────────────────────────────────────────
# Same story as the CLI tour so the two demos corroborate each other.
if [ "${KEEP_DATA:-0}" != "1" ]; then
  say "seeding $TM_DATA_DIR"
  rm -rf "$TM_DATA_DIR"; mkdir -p "$TM_DATA_DIR"

  ing() { "$CLI" ingest "$1" >/dev/null 2>&1 || true; }

  ing 'Aaditya founded TraceMind in Palo Alto in July 2026. The company builds a local memory OS.'
  ing 'https://www.fifa.com/tickets FIFA World Cup 2026 Tickets'
  ing 'Alice works at Anthropic and Bob works at Anthropic. Carol founded Anthropic.'
  ing 'TraceMind uses BGE-small ONNX embeddings and a SQLite knowledge graph.'
  ing 'The composition wedge is memory_context_for — one MCP verb every host can bind to a keystroke.'
  ing 'GEPA tunes the retrieval policy against the LoCoMo train split.'
  ing 'ColBERT reranking runs on every query via mxbai-edge-colbert-v0-17m.'
  ing 'Booked flights to Vancouver for the World Cup group stage in June 2026.'
  ing 'Rondo is a computer vision company for football scouting from phone footage.'
  ing 'JTMS truth maintenance retracts superseded beliefs when a functional predicate clashes.'

  # A supersession pair — this fires the retraction beat, which is what makes
  # Brief and Dashboard show a live contradiction rather than an empty state.
  ing 'FIFA World Cup 2026 final located in New Jersey'
  ing 'FIFA World Cup 2026 final located in Los Angeles'

  # Commitments — populates Brief, Commitments, and Ledger.
  "$CLI" commit --kind intent 'buy Group Stage tickets before the price surge' \
    --stakes high --horizon 2026-08-15T00:00:00Z --no-preflight >/dev/null 2>&1 || true
  "$CLI" commit --kind decision 'ship the MCP core-6 surface before adding new verbs' \
    --stakes high --horizon 2026-08-01T00:00:00Z --no-preflight >/dev/null 2>&1 || true
  "$CLI" commit --kind hypothesis 'the retraction beat is what makes memory feel trustworthy' \
    --stakes medium --horizon 2026-09-01T00:00:00Z --no-preflight >/dev/null 2>&1 || true

  # Queries — populates Traces with Retrieve events and gives the bandit
  # real arm statistics for Dashboard / Calibration.
  for q in 'world cup tickets' 'who works at Anthropic' 'what is the composition wedge' \
           'how does reranking work' 'what did I decide about MCP'; do
    "$CLI" query "$q" >/dev/null 2>&1 || true
  done

  # Daily note + today's backlinks — populates Home and Garden.
  "$CLI" today >/dev/null 2>&1 || true
fi

# ── 2. Arm the auto-tour ──────────────────────────────────────────────────
say "arming auto-tour (${DWELL}s per surface)"
printf '{"enabled":true,"dwell_secs":%s}\n' "$DWELL" > "$TM_DATA_DIR/demo_tour.json"

# 19 surfaces: home, query, review + 14 advanced + inspector + settings.
SURFACES=(home ask review brief composer graph ingest dashboard threads \
          views context garden events ledger commitments calibration traces \
          inspector settings)
N=${#SURFACES[@]}

# ── 3. Launch and capture ─────────────────────────────────────────────────
# Hold the display awake for the whole run. Without this, an idle Mac sleeps
# its display and `screencapture` silently returns solid-black PNGs — the
# capture succeeds, exit code 0, and you get 19 black frames. `-d` blocks
# display sleep; `-u` simulates user activity to wake it if it already slept.
caffeinate -u -t 2
caffeinate -d -i &
CAFFEINE_PID=$!

say "launching $APP"
"$APP" >/tmp/tm-demo-gui-native.log 2>&1 &
APP_PID=$!
trap 'kill "$APP_PID" "$CAFFEINE_PID" 2>/dev/null; rm -f "$TM_DATA_DIR/demo_tour.json"' EXIT

# Cold start pays for model load + first-run checks before the tour begins.
say "waiting for cold start (models + first-run check)"
sleep 12

FRAMES="$(mktemp -d)"
BLANK=0
say "capturing $N surfaces → $OUT"
for i in $(seq 0 $((N - 1))); do
  name="${SURFACES[$i]}"
  # Sample mid-dwell so we never catch a surface mid-transition.
  sleep "$(awk "BEGIN{print $DWELL/2}")"
  idx=$(printf '%02d' "$((i + 1))")
  # Name from the app's own mark file, not from our position in this loop —
  # our clock drifts against the frontend timer, which previously produced
  # frames labelled `06_graph` that actually showed Dashboard.
  marked=$(tail -1 "$TM_DATA_DIR/demo_tour_marks.jsonl" 2>/dev/null \
           | sed -n 's/.*"view":"\([a-z]*\)".*/\1/p')
  [ -n "$marked" ] && name="$marked"
  shot="$OUT/${idx}_${name}.png"
  screencapture -x -t png "$shot" 2>/dev/null
  cp "$shot" "$FRAMES/$(printf '%03d' "$i").png" 2>/dev/null

  # A slept display yields a solid-black PNG and exit code 0. Black compresses
  # to ~100KB at this resolution; a real frame is multiple MB. Fail loudly
  # rather than shipping 19 black frames again.
  sz=$(stat -f%z "$shot" 2>/dev/null || echo 0)
  if [ "$sz" -lt 500000 ]; then
    BLANK=$((BLANK + 1))
    printf '  %s✗%s %s (%s bytes — looks blank)\n' "$RED" "$RESET" "${idx}_${name}.png" "$sz"
  else
    printf '  %s✓%s %s\n' "$GREEN" "$RESET" "${idx}_${name}.png"
  fi
  sleep "$(awk "BEGIN{print $DWELL/2}")"
done

if [ "$BLANK" -gt 0 ]; then
  printf '\n%s!!%s %s of %s frames look blank — the display probably slept.\n' \
    "$RED" "$RESET" "$BLANK" "$N" >&2
  printf '   Re-run; caffeinate should prevent it. If it persists, check\n' >&2
  printf '   System Settings → Lock Screen → turn display off.\n' >&2
fi

kill "$APP_PID" 2>/dev/null
wait "$APP_PID" 2>/dev/null

# ── 4. Assemble ───────────────────────────────────────────────────────────
FFMPEG="$(command -v ffmpeg || true)"
if [ -n "$FFMPEG" ]; then
  say "assembling walkthrough video"
  "$FFMPEG" -y -framerate "1/$DWELL" -pattern_type glob -i "$FRAMES/*.png" \
    -vf "scale=1600:-2:flags=lanczos,format=yuv420p" \
    -r 30 "$OUT/product_gui_native.mp4" >/dev/null 2>&1 \
    && printf '  %s✓%s product_gui_native.mp4\n' "$GREEN" "$RESET"
  "$FFMPEG" -y -framerate "1/$DWELL" -pattern_type glob -i "$FRAMES/*.png" \
    -vf "scale=1000:-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse" \
    "$OUT/product_gui_native.gif" >/dev/null 2>&1 \
    && printf '  %s✓%s product_gui_native.gif\n' "$GREEN" "$RESET"
else
  say "ffmpeg not found — stills only"
  printf '   The Playwright-bundled build at\n' >&2
  printf '   ~/Library/Caches/ms-playwright/ffmpeg-*/ffmpeg-mac is NOT usable here:\n' >&2
  printf '   it ships only the image2 muxer and png encoder (no gif, no mp4, no x264).\n' >&2
  printf '   For video you need a full ffmpeg. Homebrew is not installed on this\n' >&2
  printf '   machine either — options: install Homebrew then `brew install ffmpeg`,\n' >&2
  printf '   or grab a static build from https://evermeet.cx/ffmpeg/ and put it on PATH.\n' >&2
fi

rm -rf "$FRAMES"
say "done → $OUT"
