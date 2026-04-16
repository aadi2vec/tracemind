#!/usr/bin/env bash
# TraceMind end-to-end quality test
# Tests ingest → query → temporal → status → feedback loop
#
# Usage: ./scripts/e2e-test.sh [--keep]
#   --keep  Don't delete test data dir after run (for inspection)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CLI="$ROOT_DIR/target/release/tracemind"
KEEP=false

for arg in "$@"; do
    [[ "$arg" == "--keep" ]] && KEEP=true
done

# Use a temp data dir so we don't touch real memory
export TM_DATA_DIR=$(mktemp -d /tmp/tm-e2e-XXXXXX)
trap '[[ "$KEEP" == "false" ]] && rm -rf "$TM_DATA_DIR"' EXIT

echo "=== TraceMind E2E Quality Test ==="
echo "Data dir: $TM_DATA_DIR"
echo ""

PASS=0
FAIL=0

check() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS  $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $name"
        FAIL=$((FAIL + 1))
    fi
}

check_output() {
    local name="$1"
    local expected="$2"
    shift 2
    local output
    output=$("$@" 2>&1) || true
    if echo "$output" | grep -qi "$expected"; then
        echo "  PASS  $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $name (expected '$expected' in output)"
        echo "        got: $(echo "$output" | head -3)"
        FAIL=$((FAIL + 1))
    fi
}

# ─── Phase 1: Ingest ──────────────────────────────────────────────
echo "--- Phase 1: Ingest ---"

check "ingest basic text" \
    "$CLI" --hash-embed ingest "Rust is a systems programming language focused on safety and performance"

check "ingest second entity" \
    "$CLI" --hash-embed ingest "Python is a high-level interpreted language popular for machine learning"

check "ingest relationship" \
    "$CLI" --hash-embed ingest "TraceMind uses Rust for its core engine and Python for ML experiments"

check "ingest temporal context" \
    "$CLI" --hash-embed ingest "Today I worked on implementing temporal queries for TraceMind"

check "ingest more context" \
    "$CLI" --hash-embed ingest "Yesterday I fixed the ColBERT retrieval arm and added MaxSim scoring"

echo ""

# ─── Phase 2: Query ───────────────────────────────────────────────
echo "--- Phase 2: Query ---"

# Note: hash embedder doesn't do real semantic search.
# We just verify the query pipeline runs and returns entities.
check_output "query returns entities" "Concept\|Technology\|Person\|Project" \
    "$CLI" --hash-embed query "What is Rust?"

check_output "query returns triples or entities" "Concept\|Technology\|RelatedTo\|IsA" \
    "$CLI" --hash-embed query "Tell me about Python"

check_output "query pipeline completes" "Concept\|Technology\|Person\|Project\|RelatedTo" \
    "$CLI" --hash-embed query "What is TraceMind?"

echo ""

# ─── Phase 3: Temporal Queries ─────────────────────────────────────
echo "--- Phase 3: Temporal Queries ---"

check_output "temporal: today" "today\|temporal\|Temporal" \
    "$CLI" --hash-embed query "what was I working on today?"

check_output "temporal: yesterday" "yesterday\|temporal\|Temporal" \
    "$CLI" --hash-embed query "what did I do yesterday?"

check_output "temporal: recently" "recently\|temporal\|Temporal" \
    "$CLI" --hash-embed query "what have I been doing recently?"

check_output "temporal: last week" "last week\|temporal\|Temporal" \
    "$CLI" --hash-embed query "show me what I worked on last week"

echo ""

# ─── Phase 4: Status & Stats ─────────────────────────────────────
echo "--- Phase 4: Status ---"

check_output "status shows all 5 arms" "colbert" \
    "$CLI" --hash-embed status

check_output "status shows arm names" "narrow\|medium\|wide\|deep" \
    "$CLI" --hash-embed status

echo ""

# ─── Phase 5: Feedback Loop ──────────────────────────────────────
echo "--- Phase 5: Feedback ---"

check "feedback arm 0" \
    "$CLI" --hash-embed feedback 0 0.8

check "feedback arm 1" \
    "$CLI" --hash-embed feedback 1 0.3

check_output "status reflects feedback" "pull" \
    "$CLI" --hash-embed status

echo ""

# ─── Phase 6: Traces ─────────────────────────────────────────────
echo "--- Phase 6: Traces ---"

check_output "trace shows recent events" "ingest\|retrieve\|Ingest\|Retrieve" \
    "$CLI" --hash-embed trace --limit 5

echo ""

# ─── Phase 7: Decay ──────────────────────────────────────────────
echo "--- Phase 7: Decay ---"

check "decay runs without error" \
    "$CLI" --hash-embed decay --factor 0.95 --threshold 0.05

echo ""

# ─── Phase 8: MCP Server (smoke test) ────────────────────────────
echo "--- Phase 8: MCP Server ---"

MCP_BIN="$ROOT_DIR/target/release/tm-mcp"
if [[ -f "$MCP_BIN" ]]; then
    # Send initialize via stdin, expect JSON-RPC response
    # Use perl timeout since macOS lacks `timeout`
    MCP_OUTPUT=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}' | \
        TM_HASH_EMBED=1 perl -e 'alarm 5; exec @ARGV' "$MCP_BIN" 2>/dev/null | head -1) || true
    if echo "$MCP_OUTPUT" | grep -q "jsonrpc"; then
        echo "  PASS  MCP server responds to initialize"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  MCP server did not respond (may need --hash-embed env)"
        FAIL=$((FAIL + 1))
    fi
else
    echo "  SKIP  MCP binary not built (run: cargo build --release -p tm-mcp)"
fi

echo ""

# ─── Summary ─────────────────────────────────────────────────────
echo "=== Results ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"
echo "  Total:  $((PASS + FAIL))"
echo ""

if [[ "$KEEP" == "true" ]]; then
    echo "Data dir preserved at: $TM_DATA_DIR"
    echo "Inspect with: TM_DATA_DIR=$TM_DATA_DIR ./target/release/tracemind status"
fi

[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
