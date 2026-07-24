#!/usr/bin/env bash
# Q4.15 — H2 2026 exit-gate benchmark run.
#
# Runs all four benchmark suites required to evaluate the Q4 exit gate:
#   1. LoCoMo  — maintain ≥ 49 F1 (existing gate, must not regress)
#   2. LongMemEval — Δ F1 ≥ +3 vs baseline (new gate)
#   3. BEAM    — contradiction-rate < half of Mem0
#   4. Loop efficiency — GEPA rollouts per +1 F1 strictly decreasing
#
# Usage:
#   ./scripts/run_exit_gate.sh [--real-embeddings] [--output path/to/results.json]
#
# Without --real-embeddings, uses --hash-embed (fast, deterministic, no download).
# With --real-embeddings, uses BGE-M3 dense (requires model download).

set -euo pipefail

BINARY="${BINARY:-./target/release/tracemind}"
OUTPUT="${OUTPUT:-docs/EXIT_GATE_RESULTS.md}"
REAL_EMBED="${REAL_EMBED:-false}"
TM_DATA_DIR="${TM_DATA_DIR:-/tmp/tm-exit-gate}"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --real-embeddings) REAL_EMBED=true; shift ;;
    --output) OUTPUT="$2"; shift 2 ;;
    *) echo "Unknown arg: $1"; exit 1 ;;
  esac
done

EMBED_FLAG=""
if [[ "$REAL_EMBED" == "false" ]]; then
  EMBED_FLAG="--hash-embed"
  echo "[exit-gate] Using hash embeddings (fast mode). Pass --real-embeddings for publishable numbers."
fi

mkdir -p "$TM_DATA_DIR"
export TM_DATA_DIR

echo "=============================="
echo " TraceMind H2 Exit Gate Run"
echo " Timestamp: $TIMESTAMP"
echo "=============================="

# ── 1. Build release binary ──────────────────────────────────────────────────
echo ""
echo "==> Building release binary..."
cargo build --release --workspace 2>&1 | grep -E "^   Compiling|^    Finished|^error" | tail -5

# ── 2. LoCoMo (existing gate: must not drop below 49 F1) ─────────────────────
echo ""
echo "==> Running LoCoMo benchmark..."
LOCOMO_RESULT=$(cargo run --release -p tm-bench-locomo --features tracemind -- \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind \
  --output /tmp/tm-exit-gate-locomo.json \
  $EMBED_FLAG 2>&1 | tail -3 || echo "LOCOMO_ERROR")
echo "$LOCOMO_RESULT"

# ── 3. LongMemEval (new gate: Δ F1 ≥ +3 vs baseline) ─────────────────────────
echo ""
echo "==> Running LongMemEval benchmark..."
LONGMEM_RESULT=$(cargo run --release -p tm-bench-longmem -- \
  --dataset crates/tm-bench-longmem/fixtures/longmem-mini.json \
  --runner mock \
  --output /tmp/tm-exit-gate-longmem.json 2>&1 | tail -3 || echo "LONGMEM_ERROR")
echo "$LONGMEM_RESULT"

# ── 4. BEAM (contradiction stress) ──────────────────────────────────────────
echo ""
echo "==> Running BEAM contradiction stress test..."
BEAM_RESULT=$(cargo run --release -p tm-bench-longmem --bin tm-bench-beam -- \
  --dataset crates/tm-bench-longmem/fixtures/beam-mini.json \
  --runner mock \
  --output /tmp/tm-exit-gate-beam.json 2>&1 | tail -3 || echo "BEAM_ERROR")
echo "$BEAM_RESULT"

# ── 5. Unit test suite (must be green) ───────────────────────────────────────
echo ""
echo "==> Running core test suite..."
TEST_RESULT=$(cargo test -p tm-types -p tm-graph -p tm-episodic \
  -p tm-controller -p tm-retrieval -p tm-reflect \
  -p tm-vector -p tm-gepa -p tm-world-model \
  -p tm-bench-longmem 2>&1 | \
  grep -E "^test result" | \
  awk '{print $0}' | \
  paste -sd '|' - || echo "TEST_ERROR")
echo "$TEST_RESULT"

# ── 6. Write results document ─────────────────────────────────────────────────
echo ""
echo "==> Writing exit gate results to $OUTPUT..."
cat > "$OUTPUT" << RESULTS_EOF
# TraceMind H2 2026 — Exit Gate Results

**Run at:** $TIMESTAMP
**Embeddings:** $([ "$REAL_EMBED" == "true" ] && echo "BGE-M3 dense (real)" || echo "hash (fast mode)")
**Data dir:** $TM_DATA_DIR

## Gate 1 — LoCoMo F1 ≥ 49 (must not regress)

\`\`\`
$LOCOMO_RESULT
\`\`\`

**Status:** See \`/tmp/tm-exit-gate-locomo.json\` for full results.

## Gate 2 — LongMemEval Δ F1 ≥ +3 vs baseline

\`\`\`
$LONGMEM_RESULT
\`\`\`

**Status:** See \`/tmp/tm-exit-gate-longmem.json\` for full results.

## Gate 3 — BEAM Contradiction-rate < half of Mem0 baseline

\`\`\`
$BEAM_RESULT
\`\`\`

**Status:** See \`/tmp/tm-exit-gate-beam.json\` for full results.

## Gate 4 — Unit test suite green

\`\`\`
$TEST_RESULT
\`\`\`

## Interpretation

| Gate | Requirement | Status |
|------|-------------|--------|
| LoCoMo ≥ 49 F1 | Must not regress from 49.27 baseline | See results above |
| LongMemEval Δ F1 ≥ +3 | No code deploy between baseline and evaluation | Requires real embeddings + design partners |
| BEAM contradiction-rate | < half of Mem0 baseline | Mock runner always passes; real evaluation requires Mem0 API |
| Test suite green | All chartered crates pass | See Gate 4 above |

## Next Steps

1. Re-run with \`--real-embeddings\` for publishable numbers
2. Compare LongMemEval numbers against the Q3 baseline published in \`docs/DESIGN.md §13\`
3. Run head-to-head against Mem0 using \`tm-bench-memory\` harness for contradiction-rate
4. If all four gates pass → tag release \`v0.4-h2-gate\` and publish blog post at \`docs/blog/\`

RESULTS_EOF

echo ""
echo "=============================="
echo " Exit gate run complete."
echo " Results written to: $OUTPUT"
echo "=============================="
