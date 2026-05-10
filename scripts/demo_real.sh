#!/usr/bin/env bash
# Real-data demo — ingests paragraphs from your actual notes (rondo
# pitch, tracemind investor brief, ad-hoc voice-style captures) and
# shows recall on real content. Designed for slow, readable recording
# with asciinema; default pacing is unhurried (3s shot, 1.5s run).
#
# Usage:
#   PAUSE_SHOT=3 PAUSE_RUN=1.5 ./scripts/demo_real.sh        # screen recording
#   PAUSE_SHOT=0 PAUSE_RUN=0   ./scripts/demo_real.sh        # CI / verify
#
# Env knobs:
#   BIN          — path to tracemind binary (default: ./target/release/tracemind)
#   TM_DATA_DIR  — data dir override (default: /tmp/tm-demo-real)
#   PAUSE_SHOT   — seconds to pause on shot heading (default: 3.0)
#   PAUSE_RUN    — seconds to pause around each command (default: 1.5)
#
# Why a separate script: the deterministic fixture (`scripts/demo_autonomous.sh`)
# proves the *retraction* beat in CI. This one demonstrates the *recall*
# beat end-to-end on real content — what a viewer would actually see if
# they sat down and started using TraceMind on day one.

set -euo pipefail

BIN="${BIN:-./target/release/tracemind}"
DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-real}"
PAUSE_SHOT="${PAUSE_SHOT:-3.0}"
PAUSE_RUN="${PAUSE_RUN:-1.5}"

SEP="────────────────────────────────────────────────────────────────"

export TM_DATA_DIR="$DATA_DIR"
rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

shot() {
  printf "\n%s\n  ▸ %s\n%s\n" "$SEP" "$1" "$SEP"
  [[ "$PAUSE_SHOT" != 0 ]] && sleep "$PAUSE_SHOT" || true
}

# Pretty `tracemind` label for the printed prompt — looks cleaner than
# the binary path on screen.
PROMPT_BIN="tracemind"

# Run prints a stylized prompt and the command, sleeps, runs (with the
# banner-on-stderr suppressed), sleeps. Multi-line strings should come
# in via stdin so the printed prompt stays one tidy line.
run() {
  printf "\n\$ %s %s\n" "$PROMPT_BIN" "$*"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
  "$BIN" "$@" 2>/dev/null
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
}

# Ingest a paragraph via stdin so the command line stays readable.
ingest_para() {
  local label="$1"
  local text="$2"
  printf "\n\$ %s ingest <<<\"%s…\"\n" "$PROMPT_BIN" "${text:0:54}"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
  printf "%s\n" "$text" | "$BIN" ingest - >/dev/null 2>&1
  printf "  ✓ %s (%d chars ingested)\n" "$label" "${#text}"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
}

# ────────────────────────────────────────────────────────────────────
shot "0 — fresh memory, nothing to recall yet"
run recent --limit 3

# ────────────────────────────────────────────────────────────────────
shot "1 — capture: a paragraph from the Rondo pitch deck"
ingest_para "Rondo · problem statement" \
"The transfer market hit \$9.6 billion in 2023, and 40% of players in Europe's top five leagues came from outside those leagues. Salah from Egypt, Mbappé from Bondy, Haaland from Norway's second division, Osimhen from Lagos. They were invisible until they weren't. Every scouting tool today requires broadcast-quality video, event data feeds, or GPS hardware — none of which exists where the next Salah is playing right now."

ingest_para "Rondo · the bet" \
"Rondo is a computer vision research company disguised as a football intelligence platform. The core bet: extract world-class player intelligence from a single phone camera on a dirt pitch. If we crack that, broadcast footage is trivial, and we see every player on earth. The breakthrough is training the perception system on synthetic data to handle the worst possible conditions — low resolution, shaky camera, no pitch lines, no kit numbers, poor lighting."

# ────────────────────────────────────────────────────────────────────
shot "2 — capture: a paragraph from the TraceMind investor brief"
ingest_para "TraceMind · the wedge" \
"TraceMind is a local-only memory OS for humans and AI agents. We are not a memory database — we are a system of intents. The primitive is a Commitment, a recorded statement of intent or decision with metadata you can predict over later. Every other memory product on the market — Mem0, Honcho, Letta, Zep — solves forgetting by sending your data to their cloud. That is a fine MVP and a terrible long-term position once memory becomes the most sensitive surface in your digital life."

ingest_para "TraceMind · the moat" \
"The user is already on a powerful machine. The right answer is to build the brain there, and only there. Cloud memory is the default; we are betting privacy is the moat. Every ingest and every query produces an immutable trace logged to disk — UUID, timestamp, content hash, entity IDs. No telemetry leaves the device. The footprint stays under 1.6 gigabytes active with the Tier-1 local LLM."

# ────────────────────────────────────────────────────────────────────
shot "3 — capture: a couple of off-the-cuff thoughts (voice-note style)"
ingest_para "voice note · pricing" \
"Two pricing pillars I keep coming back to: a free consumer tier that becomes the canonical AI-augmented Transfermarkt, and a paid pro tier for clubs, agents, and broadcasters. The free tier is the data engine. The pro tier is the revenue engine."

ingest_para "voice note · timing" \
"Why now: pose estimation hit real-time on commodity hardware in 2024, video foundation models like DINOv2 enable feature extraction without labeled data, and NIL in US college sports created a 1.2 billion dollar market with zero scouting infrastructure."

# ────────────────────────────────────────────────────────────────────
shot "4 — recall on real content: ask about the transfer market"
run query "transfer market and where the next Salah comes from"

# ────────────────────────────────────────────────────────────────────
shot "5 — recall: what is TraceMind's wedge?"
run query "what is TraceMind's wedge and why local-only"

# ────────────────────────────────────────────────────────────────────
shot "6 — cross-document recall: bridge Rondo and TraceMind"
run query "the bet on running the brain on-device"

# ────────────────────────────────────────────────────────────────────
shot "7 — pricing recall: pull the specific business detail"
run query "Rondo's pricing pillars and which tier is the data engine"

# ────────────────────────────────────────────────────────────────────
shot "8 — provenance: every ingest + query is auditable"
run trace --limit 6

printf "\n%s\n  ✓ DEMO COMPLETE — real notes, real recall, no fixture\n%s\n" "$SEP" "$SEP"
