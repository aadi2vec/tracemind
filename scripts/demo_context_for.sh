#!/usr/bin/env bash
# ------------------------------------------------------------------
# TraceMind — Composition-wedge demo (2026-07-26).
#
# One-topic story: `memory_context_for`. Every MCP-speaking host
# (Claude Code, Cursor, Goose, Windsurf) gets the same primitive —
# one keystroke → a token-budgeted, cited context brief for the
# topic you're about to type into the LLM window. This script is
# the CLI mirror of the MCP verb, so what you see is exactly what
# the host would inject.
#
# Recording
#   asciinema rec /tmp/tm-context-for.cast -c \
#     "PAUSE_SHOT=3 PAUSE_RUN=1.5 PAUSE_READ=3 scripts/demo_context_for.sh"
#   asciinema play /tmp/tm-context-for.cast
#
# Text transcript
#   scripts/demo_context_for.sh 2>&1 | tee /tmp/tm-context-for.txt
#
# Env
#   BIN         — tracemind binary (default: ./target/release/tracemind)
#   TM_DATA_DIR — data dir override (default: /tmp/tm-demo-context-for)
#   PAUSE_SHOT / PAUSE_RUN / PAUSE_READ — pacing (0 for CI runs)
# ------------------------------------------------------------------

set -euo pipefail

BIN="${BIN:-./target/release/tracemind}"
DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-context-for}"
PAUSE_SHOT="${PAUSE_SHOT:-3}"
PAUSE_RUN="${PAUSE_RUN:-1.5}"
PAUSE_READ="${PAUSE_READ:-3}"

export TM_DATA_DIR="$DATA_DIR"

if [ ! -x "$BIN" ]; then
    echo "==> building release binary (one-time)..."
    cargo build --release -p tm-cli >/dev/null 2>&1
fi

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
# 0. Fresh state.
# ------------------------------------------------------------------
rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

shot "The problem" \
     "You're mid-thread in Claude Code, need to reference last week's browsing / decisions / open commitments on a topic. Today: paste-hunt across tabs. Wedge: one MCP verb, memory_context_for(topic), returns a cited paragraph you can inject in the current chat."

# ------------------------------------------------------------------
# 1. Seed a realistic memory: browsing + a decision + a contradiction.
# ------------------------------------------------------------------
shot "Seed a realistic local memory" \
     "Simulated week of browsing on 'World Cup 2026', one Commitment ('will buy Group Stage tickets'), plus a triple that will contradict a later belief so the retraction beat has something to say."

for v in \
    "https://www.fifa.com/tickets FIFA World Cup 2026 Tickets" \
    "https://apnews.com/article/world-cup-2026-tickets Ticket resale prices for the 2026 World Cup" \
    "https://en.wikipedia.org/wiki/2026_FIFA_World_Cup 2026 FIFA World Cup — Wikipedia" \
    "https://arxiv.org/abs/2312.10997 GEPA Reflective Prompt Evolution" \
    "The 2026 FIFA World Cup final will be in New Jersey" \
    "The 2026 FIFA World Cup final will be in Los Angeles"
do
    printf '\n%s%s%s\n' "${DIM}\$ tracemind ingest \"$v\"${RESET}"
    "$BIN" ingest "$v" 2>/dev/null | grep -E '^\s*(Entities|Ingested|Trace|\[|\+)' | head -6 || true
done

run "$BIN commit --kind intent 'buy Group Stage tickets before the price surge' --stakes high --horizon 2026-08-15T00:00:00Z --no-preflight"
read_beat

# ------------------------------------------------------------------
# 2. The wedge — one command, one brief.
# ------------------------------------------------------------------
shot "One command, one brief" \
     "This is the CLI mirror of memory_context_for. Same section order (What we know → Recent activity → Open commitments → Unresolved contradictions), same [i] citations. Bindable to any keystroke in any MCP host."

run "$BIN context-for 'world cup tickets' --budget-tokens 500"
read_beat

# ------------------------------------------------------------------
# 3. Budget-aware truncation.
# ------------------------------------------------------------------
shot "Budget-aware truncation" \
     "Every host has a different context budget. Same command with a tight cap drops less-important sections and warns you it truncated — the brief always fits inside the budget it claims."

run "$BIN context-for 'world cup tickets' --budget-tokens 80"
read_beat

# ------------------------------------------------------------------
# 4. JSON payload — what an MCP host would receive.
# ------------------------------------------------------------------
shot "JSON payload — what the MCP host actually receives" \
     "Same call with --json returns entities/recent_captures/commitments/contradictions sections + ordered citations + grounding label. Local-only: no network, no telemetry, everything stays on this laptop."

run "$BIN context-for 'world cup tickets' --budget-tokens 500 --json | head -40"
read_beat

# ------------------------------------------------------------------
# 5. Grounding is honest.
# ------------------------------------------------------------------
shot "Grounding is honest" \
     "Ask for a topic memory doesn't know about — the brief says so, the host can abstain instead of hallucinating."

run "$BIN context-for 'quantum accordion practice' --budget-tokens 200"
read_beat

# ------------------------------------------------------------------
# 6. Wrap.
# ------------------------------------------------------------------
shot "Wrap" \
     "One primitive — memory_context_for(topic, budget) — turns any MCP host into a context-aware surface for your local memory. No copy-paste, no chrome, no cloud. That is the composition wedge."
