#!/usr/bin/env bash
# Real-data demo — ingests paragraphs from your actual notes (rondo
# pitch, tracemind investor brief, ad-hoc voice-style captures) into
# **named contexts** and shows scoped recall on real content, with a
# short explainer after each query.
#
# 2026-05-10 rewrite (Sprint C-0.10) — the cross-document "bridge"
# celebration is now correctly framed as a *misfeature* on a local
# device, not a flex. Same query run in two different active contexts
# returns two different scoped answers. A final shot demonstrates
# `tracemind not-related` retracting an unwanted bridge so the bandit
# learns the user's segmentation preference.
#
# Usage:
#   PAUSE_SHOT=4.5 PAUSE_RUN=2.5 PAUSE_READ=4.5 ./scripts/demo_real.sh   # screen recording
#   PAUSE_SHOT=0   PAUSE_RUN=0   PAUSE_READ=0   ./scripts/demo_real.sh   # CI / verify
#
# Env knobs:
#   BIN          — path to tracemind binary (default: ./target/release/tracemind)
#   TM_DATA_DIR  — data dir override (default: /tmp/tm-demo-real)
#   PAUSE_SHOT   — seconds to pause on a shot heading (default: 4.5)
#   PAUSE_RUN    — seconds to pause around each command (default: 2.5)
#   PAUSE_READ   — seconds to leave an explainer block on screen (default: 4.5)

set -euo pipefail

BIN="${BIN:-./target/release/tracemind}"
DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-real}"
PAUSE_SHOT="${PAUSE_SHOT:-4.5}"
PAUSE_RUN="${PAUSE_RUN:-2.5}"
PAUSE_READ="${PAUSE_READ:-4.5}"

SEP="────────────────────────────────────────────────────────────────"

export TM_DATA_DIR="$DATA_DIR"
rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

shot() {
  printf "\n%s\n  ▸ %s\n%s\n" "$SEP" "$1" "$SEP"
  [[ "$PAUSE_SHOT" != 0 ]] && sleep "$PAUSE_SHOT" || true
}

# Pretty `tracemind` label for the printed prompt.
PROMPT_BIN="tracemind"

# Run prints a stylized prompt and the command, sleeps, runs (banner-on-
# stderr suppressed), sleeps.
run() {
  printf "\n\$ %s %s\n" "$PROMPT_BIN" "$*"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
  "$BIN" "$@" 2>/dev/null
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
}

# Run, capture stdout to a variable, and also echo to screen. Avoids
# `tee /dev/tty` so the demo still runs cleanly under nohup / CI where
# stdout is not a terminal.
run_capture() {
  local var="$1"; shift
  printf "\n\$ %s %s\n" "$PROMPT_BIN" "$*"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
  local out
  out=$("$BIN" "$@" 2>/dev/null)
  printf '%s\n' "$out"
  printf -v "$var" '%s' "$out"
  [[ "$PAUSE_RUN" != 0 ]] && sleep "$PAUSE_RUN" || true
}

# Print an explainer paragraph in dim style, then pause to let the
# viewer read it. Wraps soft-wrap-friendly at ~80 cols.
explain() {
  printf "\n  \033[2m%s\033[0m\n" "── what just happened ──────────────────────────────────────"
  while IFS= read -r line; do
    printf "  \033[2m%s\033[0m\n" "$line"
  done
  printf "  \033[2m%s\033[0m\n" "────────────────────────────────────────────────────────────"
  [[ "$PAUSE_READ" != 0 ]] && sleep "$PAUSE_READ" || true
}

# Ingest a paragraph via stdin (current active context applies).
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
explain <<'EOF'
The capture ring buffer is empty — TraceMind starts every session
with no leakage from previous runs. Everything you see next is
built from scratch out of what we ingest in real time.
EOF

# ────────────────────────────────────────────────────────────────────
shot "1 — create the rondo context, then capture from the Rondo deck"
run context create rondo --tags football,cv
run context use rondo
run context current

ingest_para "Rondo · problem statement" \
"The transfer market hit \$9.6 billion in 2023, and 40% of players in Europe's top five leagues came from outside those leagues. Salah from Egypt, Mbappé from Bondy, Haaland from Norway's second division, Osimhen from Lagos. They were invisible until they weren't. Every scouting tool today requires broadcast-quality video, event data feeds, or GPS hardware — none of which exists where the next Salah is playing right now."

ingest_para "Rondo · the bet" \
"Rondo is a computer vision research company disguised as a football intelligence platform. The core bet: extract world-class player intelligence from a single phone camera on a dirt pitch. If we crack that, broadcast footage is trivial, and we see every player on earth. The breakthrough is training the perception system on synthetic data to handle the worst possible conditions — low resolution, shaky camera, no pitch lines, no kit numbers, poor lighting."

explain <<'EOF'
Notice the new step: we *named* the context. Every entity and triple
ingested while \`rondo\` is the active context gets tagged with that
context's UUID. This is the segmentation primitive — your football
notes live in a namespace, your memory-OS notes will live in
another. Local devices have *more* context blur than cloud silos,
not less. We fix that at the storage layer.
EOF

# ────────────────────────────────────────────────────────────────────
shot "2 — switch to the tracemind context, ingest the investor brief"
run context create tracemind --tags product,memory-os
run context use tracemind
run context current

ingest_para "TraceMind · the wedge" \
"TraceMind is a local-only memory OS for humans and AI agents. We are not a memory database — we are a system of intents. The primitive is a Commitment, a recorded statement of intent or decision with metadata you can predict over later. Every other memory product on the market — Mem0, Honcho, Letta, Zep — solves forgetting by sending your data to their cloud. That is a fine MVP and a terrible long-term position once memory becomes the most sensitive surface in your digital life."

ingest_para "TraceMind · the moat" \
"The user is already on a powerful machine. The right answer is to build the brain there, and only there. Cloud memory is the default; we are betting privacy is the moat. Every ingest and every query produces an immutable trace logged to disk — UUID, timestamp, content hash, entity IDs. No telemetry leaves the device. The footprint stays under 1.6 gigabytes active with the Tier-1 local LLM."

explain <<'EOF'
Two domains, two contexts. The TraceMind paragraphs are tagged with
the \`tracemind\` context UUID; the Rondo paragraphs are tagged with
the \`rondo\` one. They share no entities yet, and — by design — they
*should not* surface together by default. Decoupled-by-default is
the wedge. Coupling has to be earned.
EOF

# ────────────────────────────────────────────────────────────────────
shot "3 — a couple of off-the-cuff thoughts on Rondo pricing (voice-note style)"
run context use rondo
ingest_para "voice note · pricing" \
"Two pricing pillars I keep coming back to: a free consumer tier that becomes the canonical AI-augmented Transfermarkt, and a paid pro tier for clubs, agents, and broadcasters. The free tier is the data engine. The pro tier is the revenue engine."

ingest_para "voice note · timing" \
"Why now: pose estimation hit real-time on commodity hardware in 2024, video foundation models like DINOv2 enable feature extraction without labeled data, and NIL in US college sports created a 1.2 billion dollar market with zero scouting infrastructure."

explain <<'EOF'
Short, fragmentary captures — the shape of a real voice note, not
a polished doc. TraceMind doesn't care: same pipeline, same
provenance, same entity graph. Capture is captured. And because
the \`rondo\` context is active, these are properly scoped — they
won't pollute the TraceMind namespace tomorrow.
EOF

# ────────────────────────────────────────────────────────────────────
shot "4 — recall on real content: ask about the transfer market (rondo scope)"
run query "transfer market and where the next Salah comes from"
explain <<'EOF'
The query went through the LinUCB bandit, pulled top-K vector
matches *scoped to the rondo context*, and then 1-hop expanded the
entity graph within scope. Notice the answer surfaces Salah,
Mbappé, Osimhen — and zero TraceMind concepts. That's not a search
failure; that's *correct decoupling*.
EOF

# ────────────────────────────────────────────────────────────────────
shot "5 — switch to tracemind context, recall the wedge"
run context use tracemind
run query "what is TraceMind's wedge and why local-only"
explain <<'EOF'
The "wedge" word never appears in the ingested text — TraceMind
matched semantically on the surrounding concepts (system of intents,
local-only, the competitor list). The Tier-0 extractive backend
composed citations from the triples it surfaced; every claim has a
triple-id you can audit. Same engine as the rondo query — different
active scope, different answer.
EOF

# ────────────────────────────────────────────────────────────────────
shot "6 — scoped recall: the SAME question in two contexts → two answers"
run context use rondo
printf "\n  \033[1m(in rondo context)\033[0m\n"
run query "the bet on running the brain on-device"
explain <<'EOF'
In the rondo scope, "the bet on running the brain on-device" means
the perception system — extracting world-class player intelligence
from a single phone camera on a dirt pitch. That bet is about
*compute placement* in the CV pipeline.
EOF

run context use tracemind
printf "\n  \033[1m(in tracemind context)\033[0m\n"
run query "the bet on running the brain on-device"
explain <<'EOF'
Same query, different context, different bet. In the tracemind
scope it means privacy — building the user's memory brain on the
user's machine, never in someone else's cloud. The retriever
didn't blur these together. The answer it surfaces depends on
*who you are right now*. This is what segmentation buys you.
EOF

# ────────────────────────────────────────────────────────────────────
shot "7 — pricing recall: pull the specific business detail (rondo scope)"
run context use rondo
run query "Rondo's pricing pillars and which tier is the data engine"
explain <<'EOF'
A specific factual lookup, scoped to rondo. The voice note from
earlier had two sentences about pricing — TraceMind pulled them
out and answered which tier is which. The TraceMind investor
brief is not consulted, even though both are stored in the same
SQLite file. Co-tenant, not co-mingled.
EOF

# ────────────────────────────────────────────────────────────────────
shot "8 — when bridging IS useful: opt-in cross-context query + retraction"
printf "\n  \033[1m(rondo active; ask a question that bridges into tracemind)\033[0m\n"
run_capture xctx_out query --cross-context "the bet on running the brain on-device"
# Pull the query_id the engine printed (added in Sprint C-0.7).
QID=$(printf '%s\n' "$xctx_out" | awk '/^Query:/ {print $2; exit}')
explain <<'EOF'
With \`--cross-context\` the user explicitly asks for bridges.
Foreign-context candidates take a soft -0.15 score penalty (so
the in-scope answer still wins) but they remain visible. This is
opt-in coupling — useful when you genuinely want to see "what
parallels exist between my football work and my memory-OS work?"

Now suppose the bridge it surfaced is a stray — say it pulled in
TraceMind's privacy "bet" when you really only wanted the CV
"bet". You file a retraction:
EOF

if [[ -n "$QID" ]]; then
  # Pass a synthetic result_id label; the negative signal is keyed
  # on (query_id, result_id) and the bandit penalty applies at the
  # query level regardless of result_id specifics.
  run not-related "$QID" "tracemind_moat_bridge" --kind cross_context_bridge --weight 1.0
  explain <<'EOF'
That single command writes a row to negative_signals. The next time
a query finalises a pending bandit reward against this query_id,
the engine reads negative_weight_for_query and *subtracts* it from
the relevance reward. Arm choices that pull in stray cross-context
bridges get demoted. The wedge is not just "we let you segment" —
it's "we *learn* your segmentation from your retractions."
EOF
else
  explain <<'EOF'
(skipped not-related — could not parse query_id from stdout;
demo continues. In a recording, the user would copy-paste the
printed Query: <uuid> line.)
EOF
fi

# Re-run a follow-up query to actually finalise the pending reward
# and force the bandit to consume the negative signal. The bandit
# stats line at the end will reflect the lower mean reward.
run query "another rondo question to finalize the reward"

# ────────────────────────────────────────────────────────────────────
shot "9 — provenance: every ingest, every query, every retraction is auditable"
run trace --limit 8
explain <<'EOF'
Every line you just saw — every ingest, every query, every
not-related retraction — produced an immutable trace row: UUID,
timestamp, entity count, triple count, arm chosen, latency, plus
the active context at the time. This is the audit trail. Nothing
in TraceMind is opaque; you can replay any decision the system
made — and the user's own corrections become first-class data.
EOF

printf "\n%s\n  ✓ DEMO COMPLETE — real notes, scoped recall, learned retraction\n%s\n" "$SEP" "$SEP"
