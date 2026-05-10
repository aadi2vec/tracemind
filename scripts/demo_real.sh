#!/usr/bin/env bash
# Real-data demo — ingests paragraphs from your actual notes (rondo
# pitch, tracemind investor brief, ad-hoc voice-style captures) and
# shows recall on real content, with a short explainer after each
# query so a viewer who has never seen TraceMind can read along.
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

# Ingest a paragraph via stdin.
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
shot "1 — capture: a paragraph from the Rondo pitch deck"
ingest_para "Rondo · problem statement" \
"The transfer market hit \$9.6 billion in 2023, and 40% of players in Europe's top five leagues came from outside those leagues. Salah from Egypt, Mbappé from Bondy, Haaland from Norway's second division, Osimhen from Lagos. They were invisible until they weren't. Every scouting tool today requires broadcast-quality video, event data feeds, or GPS hardware — none of which exists where the next Salah is playing right now."

ingest_para "Rondo · the bet" \
"Rondo is a computer vision research company disguised as a football intelligence platform. The core bet: extract world-class player intelligence from a single phone camera on a dirt pitch. If we crack that, broadcast footage is trivial, and we see every player on earth. The breakthrough is training the perception system on synthetic data to handle the worst possible conditions — low resolution, shaky camera, no pitch lines, no kit numbers, poor lighting."

explain <<'EOF'
Each ingest runs a PII gate, lightweight NER, and a 384-dim BGE
embedding — entities and typed triples land in SQLite, the raw
text gets a content hash logged to traces.jsonl. No network call,
no token sent to a cloud. Two paragraphs, ~900 chars total.
EOF

# ────────────────────────────────────────────────────────────────────
shot "2 — capture: a paragraph from the TraceMind investor brief"
ingest_para "TraceMind · the wedge" \
"TraceMind is a local-only memory OS for humans and AI agents. We are not a memory database — we are a system of intents. The primitive is a Commitment, a recorded statement of intent or decision with metadata you can predict over later. Every other memory product on the market — Mem0, Honcho, Letta, Zep — solves forgetting by sending your data to their cloud. That is a fine MVP and a terrible long-term position once memory becomes the most sensitive surface in your digital life."

ingest_para "TraceMind · the moat" \
"The user is already on a powerful machine. The right answer is to build the brain there, and only there. Cloud memory is the default; we are betting privacy is the moat. Every ingest and every query produces an immutable trace logged to disk — UUID, timestamp, content hash, entity IDs. No telemetry leaves the device. The footprint stays under 1.6 gigabytes active with the Tier-1 local LLM."

explain <<'EOF'
Now the graph holds two domains: a sports-CV company (Rondo) and
a memory OS (TraceMind). They share no entities yet — but the
embedder has put them in the same vector space, and the next
queries will surface latent connections across both.
EOF

# ────────────────────────────────────────────────────────────────────
shot "3 — capture: a couple of off-the-cuff thoughts (voice-note style)"
ingest_para "voice note · pricing" \
"Two pricing pillars I keep coming back to: a free consumer tier that becomes the canonical AI-augmented Transfermarkt, and a paid pro tier for clubs, agents, and broadcasters. The free tier is the data engine. The pro tier is the revenue engine."

ingest_para "voice note · timing" \
"Why now: pose estimation hit real-time on commodity hardware in 2024, video foundation models like DINOv2 enable feature extraction without labeled data, and NIL in US college sports created a 1.2 billion dollar market with zero scouting infrastructure."

explain <<'EOF'
These are short, fragmentary captures — the shape of a real voice
note, not a polished doc. TraceMind doesn't care: same pipeline,
same provenance, same entity graph. Capture is captured.
EOF

# ────────────────────────────────────────────────────────────────────
shot "4 — recall on real content: ask about the transfer market"
run query "transfer market and where the next Salah comes from"
explain <<'EOF'
The query went through the LinUCB bandit (arm 0 — narrow), pulled
top-K vector matches, and then 1-hop expanded the entity graph.
Notice the answer: it didn't return a paragraph, it returned the
*structure* — Salah↔Egypt, Salah↔Mbappé, Lagos↔Salah, plus a
1-hop bridge to Osimhen (a name we never typed in the question).
That bridge is the graph paying off — semantic recall, not search.
EOF

# ────────────────────────────────────────────────────────────────────
shot "5 — recall: what is TraceMind's wedge?"
run query "what is TraceMind's wedge and why local-only"
explain <<'EOF'
The "wedge" word never appears in the ingested text — TraceMind
matched semantically on the surrounding concepts (system of intents,
local-only, the competitor list). The Tier-0 extractive backend
composed citations from the triples it surfaced; every claim has
a triple-id you can audit. No LLM hallucination — the answer is
literally what was stored.
EOF

# ────────────────────────────────────────────────────────────────────
shot "6 — cross-document recall: bridge Rondo and TraceMind"
run query "the bet on running the brain on-device"
explain <<'EOF'
This is the cross-domain test. "Run the brain on-device" lives in
TraceMind's moat paragraph. "The bet" framing lives in Rondo's
pitch. The retriever surfaced both — and the related-entities row
shows Rondo as a 1-hop neighbor of "football intelligence platform".
The graph stitched two unrelated documents through one shared idea.
EOF

# ────────────────────────────────────────────────────────────────────
shot "7 — pricing recall: pull the specific business detail"
run query "Rondo's pricing pillars and which tier is the data engine"
explain <<'EOF'
A specific factual lookup. The voice note from earlier had two
sentences about pricing — TraceMind pulled them out and answered
which tier is which. Same arm, same pipeline; the small captures
are first-class citizens alongside the long-form docs.
EOF

# ────────────────────────────────────────────────────────────────────
shot "8 — provenance: every ingest + query is auditable"
run trace --limit 6
explain <<'EOF'
Every line you just saw — every ingest, every query — produced an
immutable trace row: UUID, timestamp, entity count, triple count,
arm chosen, latency. This is the audit trail. Nothing in TraceMind
is opaque; you can replay any decision the system made and see
exactly what it had at the time.
EOF

printf "\n%s\n  ✓ DEMO COMPLETE — real notes, real recall, no fixture\n%s\n" "$SEP" "$SEP"
