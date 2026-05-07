#!/usr/bin/env bash
# TraceMind walkthrough — narrated, paced for video, real BGE.
#
# Self-explaining: every command has a title card before it and a recap
# after, so a viewer with zero context can follow along.
#
# Total runtime: ~6–8 min at default pace.
#
# Run from repo root:   ./demo/walkthrough.sh
# Reset state:          rm -rf /tmp/tm-walkthrough
# Faster smoke test:    DEMO_PAUSE=0.2 ./demo/walkthrough.sh

set -euo pipefail

# ============================================================================
# Config
# ============================================================================
export TM_DATA_DIR="/tmp/tm-walkthrough"
TM="./target/release/tracemind"
MCP="./target/release/tm-mcp"

# Pacing — scaled by DEMO_PAUSE (default 1.0 = "presentable").
# Total wall time at default ≈ 6–8 min.
PACE="${DEMO_PAUSE:-1.0}"
pause_title() { sleep "$(awk "BEGIN{print 5*$PACE}")"; }   # title cards
pause_read()  { sleep "$(awk "BEGIN{print 3*$PACE}")"; }   # explanatory text
pause_step()  { sleep "$(awk "BEGIN{print 1.2*$PACE}")"; } # before a command
pause_recap() { sleep "$(awk "BEGIN{print 3*$PACE}")"; }   # after a command

# ============================================================================
# Visual primitives
# ============================================================================
B=$'\033[1m'; D=$'\033[2m'; U=$'\033[4m'
RED=$'\033[31m'; GRN=$'\033[32m'; YLW=$'\033[33m'; BLU=$'\033[34m'
MAG=$'\033[35m'; CYN=$'\033[36m'; WHT=$'\033[37m'
BG_BLUE=$'\033[44m'; R=$'\033[0m'

# Width of inner content area in title cards (chars, not bytes).
INNER=68

# Pre-built borders (multi-byte safe — built character-by-character in a loop).
HBAR_DOUBLE=""
HBAR_SINGLE=""
for ((i=0; i<INNER; i++)); do HBAR_DOUBLE+="═"; HBAR_SINGLE+="─"; done

# Title card — header text + N body lines, all in a cyan double-line box.
title_card() {
  local title="$1"; shift
  printf '\n%s╔%s╗%s\n' "$CYN$B" "$HBAR_DOUBLE" "$R"
  printf '%s║%s  %-66s%s%s║%s\n' "$CYN$B" "$R$B" "$title" "$R" "$CYN$B" "$R"
  printf '%s╠%s╣%s\n' "$CYN$B" "$HBAR_DOUBLE" "$R"
  for line in "$@"; do
    printf '%s║%s  %-66s%s║%s\n' "$CYN$B" "$R" "$line" "$CYN$B" "$R"
  done
  printf '%s╚%s╝%s\n\n' "$CYN$B" "$HBAR_DOUBLE" "$R"
}

# Step header — small banner before each command block.
step_header() {
  printf '\n%s%s▸ %s%s\n' "$B" "$GRN" "$1" "$R"
  printf '%s  %s%s\n' "$D" "$HBAR_SINGLE" "$R"
}

# Inline narration line (dim grey).
narrate() { printf '%s  %s%s\n' "$D" "$1" "$R"; }

# Highlighted recap callout after a command.
RECAP_TOP="┌─ what you just saw ────────────────────────────────────────────────"
RECAP_BOT="└────────────────────────────────────────────────────────────────────"
recap_open()  { printf '\n%s%s%s%s\n' "$B" "$MAG" "$RECAP_TOP" "$R"; }
recap_line()  { printf '%s%s│%s %s\n' "$B" "$MAG" "$R" "$1"; }
recap_close() { printf '%s%s%s%s\n' "$B" "$MAG" "$RECAP_BOT" "$R"; }

# Run a shell command, showing it first like a prompt.
run() {
  printf '\n%s$ %s%s\n' "$YLW$B" "$1" "$R"
  pause_step
  eval "$1"
}

# ============================================================================
# Pre-flight
# ============================================================================
if [[ ! -x "$TM" || ! -x "$MCP" ]]; then
  echo "${RED}Build first:${R}  cargo build --release --workspace --exclude tm-tauri" >&2
  exit 1
fi
rm -rf "$TM_DATA_DIR"
mkdir -p "$TM_DATA_DIR"

# ============================================================================
# COLD OPEN
# ============================================================================
clear || true

cat <<EOF


${B}${CYN}    ████████╗██████╗  █████╗  ██████╗███████╗███╗   ███╗██╗███╗   ██╗██████╗${R}
${B}${CYN}    ╚══██╔══╝██╔══██╗██╔══██╗██╔════╝██╔════╝████╗ ████║██║████╗  ██║██╔══██╗${R}
${B}${CYN}       ██║   ██████╔╝███████║██║     █████╗  ██╔████╔██║██║██╔██╗ ██║██║  ██║${R}
${B}${CYN}       ██║   ██╔══██╗██╔══██║██║     ██╔══╝  ██║╚██╔╝██║██║██║╚██╗██║██║  ██║${R}
${B}${CYN}       ██║   ██║  ██║██║  ██║╚██████╗███████╗██║ ╚═╝ ██║██║██║ ╚████║██████╔╝${R}
${B}${CYN}       ╚═╝   ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝╚══════╝╚═╝     ╚═╝╚═╝╚═╝  ╚═══╝╚═════╝${R}

         ${B}A local-only memory OS · system of intents · world model${R}

EOF
pause_read
narrate "Everything you're about to see runs on this machine."
narrate "No cloud. No telemetry. No remote model calls."
pause_read
narrate "Five acts, ~7 minutes:"
printf '%s    1. Memory          (real BGE embeddings, real entity extraction)%s\n' "$D" "$R"
printf '%s    2. Intents         (commitment as a first-class object)%s\n' "$D" "$R"
printf '%s    3. World model     (predicts outcomes from your own track record)%s\n' "$D" "$R"
printf '%s    4. Calibration     (the model only speaks when it has earned trust)%s\n' "$D" "$R"
printf '%s    5. MCP             (same brain, any agent runtime)%s\n' "$D" "$R"
pause_title

# ============================================================================
# ACT 1 — MEMORY
# ============================================================================
title_card "ACT 1 of 5 — Memory" \
  "" \
  "TraceMind ingests free-form text. For each fact, three" \
  "things happen, all on-device:" \
  "" \
  "  1. GLiNER NER pulls named entities (Person, Product, …)" \
  "  2. BGE-small ONNX embeds the text into a 384-dim vector" \
  "  3. SQLite persists the entity, the vector, and an audit trace" \
  "" \
  "Watch the [Entity] list and Trace ID after each call."
pause_title

step_header "Step 1/4 — ingest a fact about the world-model crate"
narrate "We tell TraceMind a real fact from this codebase. One sentence."
pause_read

run "$TM ingest 'TM-INTENT-003 added the tm-world-model crate as the first predictive layer in the brain architecture, predicting outcome polarity from commitment metadata.'"

recap_open
recap_line "Pipeline: text → GLiNER NER → BGE-small ONNX → SQLite, ~50 ms total."
recap_line "GLiNER labels recognised entities by type ([Product], [Person], …)."
recap_line "Trace ID above is your audit trail — every ingest is provenance-tracked."
recap_close
pause_recap

step_header "Step 2/4 — ingest a fact about the daily brief"
narrate "TraceMind has a 'daily brief' surface. This explains how the world"
narrate "model became part of it. Same pipeline, different sentence."
pause_read

run "$TM ingest 'TM-INTENT-005 promoted the world model to ambient: the daily brief now includes an outlook line, and the model auto-retrains every time an outcome is resolved.'"

recap_open
recap_line "Entity vocabulary is growing: now we have INTENT-003 and INTENT-005,"
recap_line "linked through co-occurrence in the knowledge graph."
recap_close
pause_recap

step_header "Step 3/4 — a fact about calibration"
narrate "Plant a third fact about how trust is earned in the predictive layer."
pause_read

run "$TM ingest 'TM-INTENT-010 closed the trust loop with calibration auto-quiet: when Brier score climbs, the world-model outlook silences itself until calibration recovers.'"
pause_recap

step_header "Step 4/4 — a fact about TraceMind's product surfaces"
narrate "Last one: the high-level shape of what TraceMind is."
pause_read

run "$TM ingest 'TraceMind ships as a CLI binary (tracemind), an MCP server (tm-mcp) exposing 21 JSON-RPC tools, a capture daemon, and a Tauri desktop app.'"
pause_recap

# ----- query -----
step_header "Now query — find what we just stored, semantically"
narrate "We will ask: 'how does the world model earn trust over time?'"
narrate "Watch the routing: it should land on TM-INTENT-010 (calibration auto-quiet),"
narrate "because that's the fact that's actually about earning trust."
pause_read

run "$TM query 'how does the world model earn trust over time'"

recap_open
recap_line "Top-ranked memory: TM-INTENT-010 — exactly the fact about trust."
recap_line "This is real semantic retrieval, not keyword matching: nothing in"
recap_line "the query says 'calibration' or 'auto-quiet'. BGE embeddings did the work."
recap_line ""
recap_line "Notice the [arm=N, latency=Nms] in the trace: that's the contextual"
recap_line "bandit picking which retrieval pipeline to run for this query."
recap_close
pause_recap; pause_recap

# ============================================================================
# ACT 2 — INTENTS
# ============================================================================
title_card "ACT 2 of 5 — System of intents" \
  "" \
  "A commitment is the wedge primitive. It is NOT a note;" \
  "it is NOT a todo. It is a recorded statement of intent" \
  "or decision, with metadata you can predict over later:" \
  "" \
  "    kind        intent | decision | hypothesis" \
  "    stakes      low | medium | high | reversible" \
  "    horizon     when this should resolve by" \
  "    expected    what 'success' looks like" \
  "" \
  "Then you resolve it — and the polarity becomes training data."
pause_title

step_header "Step 1/4 — empty brief, before any commitments"
narrate "The daily brief is the calm-mode surface. With no commitments yet"
narrate "it just shows zeros. No alarms, no nagging."
pause_read

run "$TM brief"
pause_recap

step_header "Step 2/4 — record a real intent"
narrate "We commit to opening a PR for this very work. Notice the metadata:"
narrate "kind=intent, stakes=medium, confidence=0.85, tags=tracemind,merge,ci."
pause_read

_COMMIT_OUT=$($TM commit \
  --kind intent \
  --stakes medium \
  --confidence 0.85 \
  --tags 'tracemind,merge,ci' \
  --no-preflight \
  'Open a PR merging claude/locomo-results into main and let CI run before merging' 2>&1)
printf '%s\n' "$_COMMIT_OUT"
PR_ID=$(printf '%s\n' "$_COMMIT_OUT" | awk '/^commitment/ {print $2; exit}')
narrate "captured commitment_id=${PR_ID}"
pause_recap

step_header "Step 3/4 — show open commitments"
narrate "It moved into 'open' state. State machine: open → acted → completed."
pause_read

run "$TM commitments"

recap_open
recap_line "One open commitment, ID prefix matches what we just captured."
recap_line "This is the only thing on our 'mind' right now."
recap_close
pause_recap

step_header "Step 4/4 — resolve it with an outcome"
narrate "Real life: the PR opens, CI is green, it merges. We tell TraceMind:"
narrate "polarity = as_expected, free-text description of what happened."
pause_read

run "$TM resolve --polarity as_expected ${PR_ID} 'PR opened; CI green; merged cleanly'"

recap_open
recap_line "State machine walked open → completed (as_expected)."
recap_line "An Outcome row was created with timestamp and polarity."
recap_line "This single (commitment + outcome) pair is now training data point #1."
recap_close
pause_recap

step_header "Brief, again — now with one resolved commitment"
narrate "Same brief command. Different output: the resolved row appears."
pause_read

run "$TM brief"
pause_recap; pause_recap

# ============================================================================
# ACT 3 — WORLD MODEL
# ============================================================================
title_card "ACT 3 of 5 — World model" \
  "" \
  "f_outcome: a tiny logistic regression that predicts polarity" \
  "(better / as_expected / worse / mixed) from commitment metadata." \
  "" \
  "Features it sees:        Features it does NOT see:" \
  "  · stakes                 · the statement text" \
  "  · kind                   · embeddings" \
  "  · horizon band           · external priors" \
  "  · time-of-day band" \
  "  · tag bag-of-words" \
  "" \
  "It needs ≥6 priors to start predicting. We bootstrap now."
pause_title

step_header "Step 1/3 — bootstrap with 5 more (commitment + outcome) pairs"
narrate "Mixed polarities so the model has signal to learn from."
narrate "These resolve instantly for the demo. Real usage: weeks/months."
pause_read

mk_pair() {
  local kind="$1" stakes="$2" pol="$3" tags="$4" stmt="$5"
  local id
  id=$($TM commit --kind "$kind" --stakes "$stakes" --confidence 0.8 --tags "$tags" --no-preflight "$stmt" 2>&1 | awk '/^commitment/ {print $2}')
  $TM resolve --polarity "$pol" "$id" "auto-resolved for demo bootstrap" >/dev/null
  printf '  %s· %s%-26s%s %s\n' "$D" "$BLU" "[$kind/$stakes/$pol]" "$D" "${stmt:0:38}…$R"
}

mk_pair intent     low      as_expected "tracemind,docs"   "Update LOCOMO_RESULTS.md with v0.2 numbers"
mk_pair intent     medium   as_expected "tracemind,build"  "Pull Tier 1 LLM weights so ask returns prose"
mk_pair decision   medium   as_expected "tracemind,arch"   "Use BGE-small-en-v1.5 as the default embedder"
mk_pair intent     high     worse       "tracemind,bench"  "Run full LoCoMo dataset overnight on CI"
mk_pair hypothesis low      as_expected "tracemind,nlp"    "GLiNER picks up TM-INTENT-### as Products"
pause_recap

step_header "Step 2/3 — train the world model"
narrate "Logistic regression. 22-dim feature space. SGD with L2 regularization."
narrate "On 6 examples with 2 classes it converges in milliseconds."
pause_read

run "$TM world train"

recap_open
recap_line "Trained on 6 priors → 100% in-sample accuracy (small set)."
recap_line "Weights persisted to ~/.tracemind/world_model.json (~3 KB)."
recap_line "The schema is versioned: future model upgrades can migrate or refuse."
recap_close
pause_recap

step_header "Step 3/3 — preflight: the magic moment"
narrate "We're about to commit a NEW intent that smells like past failures:"
narrate "high-stakes + 'tracemind,scale' tags + 'full LoCoMo' phrase."
narrate "Watch what happens before TraceMind even saves the commitment…"
pause_read; pause_read

run "$TM commit --kind intent --stakes high --confidence 0.7 --tags 'tracemind,scale' 'Ship full LoCoMo evaluation gate in CI before next release'"

recap_open
recap_line "⚠ track record (n=6): the model just predicted this lands 'worse'."
recap_line ""
recap_line "It did NOT block the commitment. The commit still saved."
recap_line "It just put a one-line warning above the confirmation."
recap_line "Your call. The system gives you data, not orders."
recap_close
pause_recap; pause_recap

# ============================================================================
# ACT 4 — CALIBRATION
# ============================================================================
title_card "ACT 4 of 5 — Calibration" \
  "" \
  "A predictor that's never wrong on the training set is" \
  "easy to build. A predictor you should TRUST is harder." \
  "" \
  "Calibration = Brier score + per-class reliability table," \
  "scored only on commitments resolved AFTER the last train." \
  "Out-of-sample only. No in-sample self-congratulation." \
  "" \
  "If the score drifts, the brief auto-quiets the outlook line." \
  "The model has to earn the screen back."
pause_title

step_header "View calibration — honest empty state"
narrate "We just trained, then immediately ran calibration. So nothing is"
narrate "out-of-sample yet. The system is honest about that — no fake numbers."
pause_read

run "$TM world calibration"

recap_open
recap_line "n_evaluated=0 because every prior is in-sample."
recap_line "As you resolve more commitments AFTER training, this populates."
recap_line "If the model drifts, the brief stops showing the outlook line."
recap_line "The user stays in control of when predictions get screen real estate."
recap_close
pause_recap; pause_recap

# ============================================================================
# ACT 5 — MCP
# ============================================================================
title_card "ACT 5 of 5 — MCP server" \
  "" \
  "Same brain. Different agent runtime." \
  "" \
  "TraceMind exposes its full surface as a Model Context Protocol" \
  "server over JSON-RPC stdio. Any MCP-aware client (Claude Code," \
  "Cursor, custom agents) can wire up these tools." \
  "" \
  "Local-only stays true: stdio is in-process, no network."
pause_title

step_header "List the tools"
narrate "Send a single JSON-RPC request: tools/list."
narrate "Get back the full tool catalogue."
pause_read

printf '\n%s$ echo {"jsonrpc":"2.0","id":1,"method":"tools/list"} | tm-mcp%s\n' "$YLW$B" "$R"
pause_step
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | "$MCP" 2>/dev/null | \
  python3 -c '
import json, sys
r = json.loads(sys.stdin.read())
tools = r["result"]["tools"]
print("  " + str(len(tools)) + " tools exposed:")
for t in tools:
    print("    \u00b7 " + t["name"])
'

recap_open
recap_line "21 tools, mirroring the CLI surface plus a few MCP-only helpers."
recap_line "Any agent framework that speaks MCP gets full read+write access."
recap_line "The brain stays on this machine. The interface is universal."
recap_close
pause_recap; pause_recap

# ============================================================================
# CLOSE
# ============================================================================
title_card "Final state" \
  "" \
  "One last brief — the populated steady-state view." \
  "Resolved commitments, the new open commitment with its preflight," \
  "the model's warming-up insight panel."
pause_title

run "$TM brief"
pause_recap

cat <<EOF


${B}${CYN}    ╭───────────────────────────────────────────────────────────────────╮${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${B}Local. Yours. The brain is the product.${R}                        ${B}${CYN}│${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}Everything you saw runs on-device.${R}                             ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}Embeddings, retrieval, the world model, the audit trail.${R}      ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}~/.tracemind/ is the only state. You can rm -rf it.${R}            ${B}${CYN}│${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    ╰───────────────────────────────────────────────────────────────────╯${R}


EOF
pause_title
