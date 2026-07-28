#!/usr/bin/env bash
# ------------------------------------------------------------------
# TraceMind — Full product tour (2026-07-26).
#
# End-to-end walkthrough covering every surface of the local memory
# OS: ingestion (text, URL, bulk, ambient capture), memory backend
# (bandit-routed retrieval, tiered answers, quick-recall, trace
# log), graph features (backlinks, contexts, views, pending, today,
# contradictions + retraction), the System of Intents (commit /
# resolve / brief), the composition wedge (context-for), self-
# improvement (status / policy / nightly), and the privacy story
# (export + storage). Roughly 4-5 minutes at default pacing.
#
# Recording
#   asciinema rec /tmp/tm-product-tour.cast -c \
#     "PAUSE_SHOT=3.5 PAUSE_RUN=1.5 PAUSE_READ=3 scripts/demo_product_tour.sh"
#   asciinema play  /tmp/tm-product-tour.cast
#   asciinema upload /tmp/tm-product-tour.cast   # optional
#
# Text transcript (no ANSI): PAUSE_*=0 for CI.
#   scripts/demo_product_tour.sh 2>&1 | tee /tmp/tm-product-tour.txt
#
# Env
#   BIN         — tracemind binary (default: ./target/release/tracemind)
#   TM_DATA_DIR — data dir override (default: /tmp/tm-demo-tour)
#   PAUSE_SHOT / PAUSE_RUN / PAUSE_READ — pacing (0 for CI runs)
#
# Everything runs on-device with real BGE embeddings. No network.
# ------------------------------------------------------------------

set -euo pipefail

BIN="${BIN:-./target/release/tracemind}"
DATA_DIR="${TM_DATA_DIR:-/tmp/tm-demo-tour}"
IMPORT_DIR="${IMPORT_DIR:-/tmp/tm-demo-tour-import}"
EXPORT_DIR="${EXPORT_DIR:-/tmp/tm-demo-tour-export}"
PAUSE_SHOT="${PAUSE_SHOT:-3.5}"
PAUSE_RUN="${PAUSE_RUN:-1.5}"
PAUSE_READ="${PAUSE_READ:-3}"

export TM_DATA_DIR="$DATA_DIR"

if [ ! -x "$BIN" ]; then
    echo "==> building release binary (one-time)..."
    cargo build --release -p tm-cli >/dev/null 2>&1
fi

BOLD=$'\033[1m'; DIM=$'\033[2m'
CYAN=$'\033[36m'; YEL=$'\033[33m'; GRN=$'\033[32m'; MAG=$'\033[35m'
RESET=$'\033[0m'

section() {
    printf '\n%s\n' "${BOLD}${MAG}════════════════════════════════════════════════════════${RESET}"
    printf '%s\n'   "${BOLD}${MAG} $1${RESET}"
    printf '%s\n'   "${BOLD}${MAG}════════════════════════════════════════════════════════${RESET}"
    sleep "$PAUSE_SHOT"
}
shot() {
    printf '\n%s\n' "${BOLD}${CYAN}▸ $1${RESET}"
    printf '%s\n' "${DIM}$2${RESET}"
    sleep "$PAUSE_SHOT"
}
run() {
    printf '\n%s%s%s\n' "${YEL}\$ " "$1" "${RESET}"
    sleep "$PAUSE_RUN"
    # shellcheck disable=SC2086
    eval "$1" || true
    sleep "$PAUSE_RUN"
}
note()      { printf '\n%s%s%s\n' "${GRN}» " "$1" "${RESET}"; }
read_beat() { sleep "$PAUSE_READ"; }

# ==================================================================
# 0. Fresh state.
# ==================================================================
rm -rf "$DATA_DIR" "$IMPORT_DIR" "$EXPORT_DIR"
mkdir -p "$DATA_DIR" "$IMPORT_DIR"

# ==================================================================
section "ACT I — What is TraceMind?"
# ==================================================================
shot "The pitch" \
     "TraceMind is a local-only memory OS. Everything you're about to see runs on this laptop: real ONNX embeddings (BGE-small-384d), a SQLite graph, a JTMS truth-maintenance layer, ambient capture, tiered answers, and an MCP surface that plugs into Claude Code / Cursor / Goose. No cloud, no telemetry, ~/.tracemind/ is the whole footprint."

run "$BIN onboard --dry-run"
read_beat

# ==================================================================
section "ACT II — Ingestion, every shape"
# ==================================================================

shot "1. Text ingest" \
     "Plain text goes through governance (regex PII + confidence gate) → heuristic NER (URLs, emails, dates, entity typing) → SQLite graph upsert → BGE embedding → immutable Trace event."

run "$BIN ingest 'Aaditya founded TraceMind in Palo Alto in July 2026. The company builds a local memory OS.'"
read_beat

shot "2. URL ingest (browser-shaped)" \
     "URLs get first-class treatment: raw URL becomes a Url entity AND the registrable domain becomes an Organization node. That means a query for 'FIFA' hits the graph directly, not just BGE similarity."

for v in \
    "https://www.fifa.com/tickets FIFA World Cup 2026 Tickets" \
    "https://apnews.com/article/world-cup-2026-tickets Ticket resale prices for the 2026 World Cup" \
    "https://arxiv.org/abs/2312.10997 GEPA Reflective Prompt Evolution" \
    "https://superlinked.com/vectorhub Vector Hub — Superlinked"
do
    printf '\n%s%s%s\n' "${DIM}\$ tracemind ingest \"$v\"${RESET}"
    "$BIN" ingest "$v" 2>/dev/null | grep -E '^\s*(Entities|Ingested|Trace|\[|\+)' | head -6 || true
done
read_beat

shot "3. Bulk import — a directory of files" \
     "Point 'tracemind import' at any folder. Each file becomes a Trace with content-addressed provenance. Ideal for onboarding an existing knowledge base."

mkdir -p "$IMPORT_DIR"
cat > "$IMPORT_DIR/roadmap.md" <<'EOF'
# TraceMind H2 2026 Roadmap
- Composition wedge: memory_context_for MCP verb
- Retraction beat via JTMS
- Nightly on-device self-improvement
EOF
cat > "$IMPORT_DIR/design-note.md" <<'EOF'
# Design note: bandit-routed retrieval
Five arms (narrow / medium / wide / deep / colbert). LinUCB with 384-d context.
State persists to bandit.json + linucb.json.
EOF
cat > "$IMPORT_DIR/investor-brief.md" <<'EOF'
# Investor brief
TraceMind is a local-only memory OS. Consumer wedge: composition across LLM chats.
EOF

run "$BIN import '$IMPORT_DIR'"
read_beat

shot "4. Ambient capture — opt-in per source" \
     "Clipboard, shell history, screenshots, browser history, audio, calendar. Every source is opt-in and revocable. Default first-run: clipboard + shell on, everything else off. State persists to ~/.tracemind/capture_permissions.toml."

run "$BIN capture list"
read_beat

shot "5. Capture doctor — the honest permission story" \
     "Reports which sources are actually reachable right now. macOS Safari needs Full Disk Access; the doctor tells you exactly what to grant. On builds with --features macos-screencapture, it also shows whether Vision-framework OCR is compiled in."

run "$BIN capture-doctor 2>&1 | head -25"
read_beat

# ==================================================================
section "ACT III — Memory backend"
# ==================================================================

shot "6. Query — bandit-routed retrieval" \
     "The query planner classifies the intent; a contextual bandit (LinUCB with 5 arms — narrow / medium / wide / deep / colbert) picks the retrieval width; results fuse dense (BGE) + BM25 lexical + recency + confidence via ComposedIndex, then ColBERT MaxSim reranks."

run "$BIN query 'world cup tickets'"
read_beat

shot "7. Ask — tiered answer layer" \
     "Extractive Tier 0 always works. Tier 1 (Qwen 2.5 1.5B Q4 GGUF, ~900 MB) and Tier 2 (Apple FoundationModels on macOS 26+) are opt-in features. Answers ground on retrieved entities with citations — abstains rather than hallucinating."

run "$BIN ask 'where can I buy tickets for the 2026 World Cup?'"
read_beat

shot "8. Quick recall — launcher target (X2/X8)" \
     "Sub-100ms, small top_k, no LLM. Bind this to a global shortcut in Raycast / Alfred: keystroke → the site you're thinking of, no chrome."

run "$BIN quick-recall 'reflective prompt' --top-k 3"
read_beat

shot "9. Trace — immutable audit trail" \
     "Every ingest + query produces a Trace (UUID, timestamp, content hash, entity IDs) written append-only to traces.jsonl. Ground truth for provenance and rollback."

run "$BIN trace --limit 6"
read_beat

# ==================================================================
section "ACT IV — Graph features"
# ==================================================================

shot "10. Backlinks — LM-1" \
     "Every entity has a typed backlink panel: which entities link *to* it, with predicate + confidence. The graph is the primary UI, not vectors."

# Seed a target entity with multiple incoming edges, then look it up.
"$BIN" ingest 'Alice works at Anthropic and Bob works at Anthropic. Carol founded Anthropic.' >/dev/null 2>&1 || true

set +e
# `today` prints `- [Type] Name  _(conf ..., id <uuid>)_` for every
# memory linked to today's DailyNote. Grab the Anthropic row's id.
TARGET_ENTITY=$(
    "$BIN" today 2>/dev/null \
    | grep -F 'Anthropic' \
    | grep -oE 'id [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' \
    | head -1 \
    | awk '{print $2}'
)
set -e
if [ -n "$TARGET_ENTITY" ]; then
    run "$BIN backlinks $TARGET_ENTITY"
else
    note "(no matching entity for backlinks scene — skipped)"
fi
read_beat

shot "11. Contexts — decoupled by default (C-0)" \
     "Named namespaces. Every signal + triple inherits the active context_id at ingest time. Retrieval defaults to the active scope. The point: your local machine has MORE context blur than a browser — decoupling is the wedge."

run "$BIN context list"
run "$BIN context create work"
run "$BIN context list"
read_beat

shot "12. Memory Views — LM-11" \
     "Saved include/exclude splices of memory. Surgical control over which memories enter which session. Karpathy-style PKM: you own the graph."

run "$BIN view list"
read_beat

shot "13. Pending relations — LM-9" \
     "Mid-confidence extractions land in a pending pool, waiting for confirmation. Nothing enters trusted memory without a decision path."

run "$BIN pending list"
read_beat

shot "14. Today — Karpathy daily note (LM-5c)" \
     "Idempotent DailyNote entity for today's local date, auto-linked to every memory created today. Zero manual bookkeeping."

run "$BIN today"
read_beat

# ==================================================================
section "ACT V — Contradictions + retraction beat (JTMS)"
# ==================================================================

shot "15. The retraction beat (JTMS supersession)" \
     "Ingest two conflicting facts with the same subject + predicate. The JTMS truth-maintenance layer detects the collision and the retraction beat surfaces a one-line reconcile message on the newer belief — memory doesn't silently accumulate contradictions."

run "$BIN ingest 'FIFA World Cup 2026 final located in New Jersey'"
run "$BIN ingest 'FIFA World Cup 2026 final located in Los Angeles'"

note "Watch the second ingest — the ⚠ Retraction line above is the beat firing. That message is what the MCP host / Tauri brief also shows."
run "$BIN contradictions list"
read_beat

# ==================================================================
section "ACT VI — System of Intents"
# ==================================================================

shot "16. Commit an intent" \
     "The wedge primitive of the intent system. A Commitment has kind (intent/decision/hypothesis), horizon, stakes, and expected outcome. Everything downstream — outcomes, patterns, insights, the world model — hangs off this."

run "$BIN commit --kind intent 'buy Group Stage tickets before the price surge' --stakes high --horizon 2026-08-15T00:00:00Z --no-preflight"
read_beat

shot "17. Daily brief" \
     "Overdue + open + recently resolved + pending candidates. This is what a menu-bar surface would badge in the morning."

run "$BIN brief"
read_beat

shot "18. Commitments list" \
     "Every open (non-terminal) commitment with its state. Ready for outcome attachment."

run "$BIN commitments"
read_beat

# ==================================================================
section "ACT VII — Composition wedge (memory_context_for)"
# ==================================================================

shot "19. One primitive, every LLM host" \
     "This is the innovation: memory_context_for(topic, budget) — a token-budgeted, cited context brief bindable to one keystroke in Claude Code / Cursor / Goose / Windsurf. Same primitive on the MCP surface. What follows is the CLI mirror."

run "$BIN context-for 'world cup tickets' --budget-tokens 500"
read_beat

shot "20. Budget-aware truncation" \
     "Tight budget → drops less-important sections and warns you. The brief always fits inside the budget it claims."

run "$BIN context-for 'world cup tickets' --budget-tokens 80"
read_beat

# ==================================================================
section "ACT VIII — Self-improvement (on-device)"
# ==================================================================

shot "21. Bandit arm statistics" \
     "The retrieval bandit exposes its arms and their reward moments — no hidden state, no cloud aggregation."

run "$BIN status"
read_beat

shot "22. Retrieval policy — GEPA-tuned, promotable" \
     "The GEPA loop runs against the real stack and produces a candidate RetrievalPolicy (ComposedIndex weights + score floor + answer weights). tracemind policy set <report> promotes it in-place. tracemind rollback list reverses any promotion."

run "$BIN policy show"
read_beat

shot "23. Nightly on-device self-improvement (Q4.4)" \
     "Runs on the laptop: GEPA spike, verb affinity update, tier cycle, contradiction-rate. Results persisted to ~/.tracemind/nightly_runs.jsonl. Nothing leaves the device."

run "$BIN nightly --help 2>&1 | head -12"
read_beat

# ==================================================================
section "ACT IX — Privacy story (LM-16)"
# ==================================================================

shot "24. Export — your memory is yours" \
     "The whole graph as a portable Obsidian-compatible markdown bundle: one file per entity with [[wikilinks]] + index.md. Trust artifact and data path for PKM workflows."

run "$BIN export --output '$EXPORT_DIR'"
run "ls '$EXPORT_DIR' | head -10"
run "wc -l '$EXPORT_DIR'/index.md 2>/dev/null || true"
read_beat

shot "25. Storage report" \
     "Local-only cleanups (vacuum, prune ephemeral signals, truncate the trace log). Never touches the network."

run "$BIN storage status 2>&1 | head -20"
read_beat

# ==================================================================
section "ACT X — Wrap"
# ==================================================================

shot "26. Weekly digest" \
     "Every week the daemon summarizes captures by modality + unresolved contradictions + open commitments to ~/.tracemind/notifications/ so a menu-bar app can badge it."

run "$BIN digest --recent-days 7"
read_beat

shot "That was TraceMind." \
     "Local-only memory OS. Ambient ingestion (text/URL/bulk/browser/screenshot). Bandit-routed retrieval + tiered answers + immutable trace log. Graph features: backlinks / contexts / views / pending / today. Contradictions with a retraction beat. System of Intents. Composition primitive for every MCP host. Nightly on-device self-improvement. Full export. Every byte stayed on this laptop."
