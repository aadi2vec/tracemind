#!/usr/bin/env bash
# UI-7 — Sprint D demo smoke test. Exercises the full investor-demo flow
# end-to-end from the CLI so we can verify the surface before recording.
#
# Mirrors the Tauri click-path:
#   1. demo restore  → two contexts, fixture entities/triples, contradiction
#   2. context list  → both contexts visible
#   3. context use "Mercury work"
#   4. ingest scoped seed text into Mercury
#   5. context use "TraceMind dev"
#   6. ingest scoped seed text into Dev
#   7. context use "Mercury work" + query  → scoped result set
#   8. helpful  (👍 — F-1 positive signal)
#   9. not-related  (👎 — C-0.7 negative signal)
#  10. brief → contradiction + commitments still surface
#
# Usage: ./scripts/demo_smoke.sh [--keep]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CLI="$ROOT_DIR/target/release/tracemind"
KEEP=false

for arg in "$@"; do
    [[ "$arg" == "--keep" ]] && KEEP=true
done

if [[ ! -x "$CLI" ]]; then
    echo "✗ tracemind binary missing at $CLI — run 'cargo build -p tm-cli --release' first." >&2
    exit 1
fi

export TM_DATA_DIR=$(mktemp -d /tmp/tm-demo-smoke-XXXXXX)
trap '[[ "$KEEP" == "false" ]] && rm -rf "$TM_DATA_DIR"' EXIT

PASS=0
FAIL=0
check() {
    local label="$1"; shift
    if "$@"; then
        echo "  ✓ $label"
        PASS=$((PASS + 1))
    else
        echo "  ✗ $label"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== UI-7 Demo Smoke ==="
echo "Data dir: $TM_DATA_DIR"
echo

echo "[1] demo restore"
"$CLI" demo restore >/tmp/tm-smoke-restore.out 2>&1
check "two contexts seeded" grep -q "contexts:.*2" /tmp/tm-smoke-restore.out
check "15 entities seeded"  grep -q "entities:.*15" /tmp/tm-smoke-restore.out
check "contradiction seeded" grep -q "contradictions:.*1" /tmp/tm-smoke-restore.out

echo
echo "[2] context list"
"$CLI" context list > /tmp/tm-smoke-ctx.out 2>&1
check "Mercury work visible"   grep -q "Mercury work"   /tmp/tm-smoke-ctx.out
check "TraceMind dev visible"  grep -q "TraceMind dev"  /tmp/tm-smoke-ctx.out

echo
echo "[3..6] scoped ingest"
"$CLI" context use "Mercury work" >/dev/null
"$CLI" --hash-embed ingest "Mercury contract negotiation with Alice and Bob about Q1 board memo" >/dev/null
"$CLI" --hash-embed ingest "Carla collaborates with Alice on the Mercury onboarding doc" >/dev/null
"$CLI" context use "TraceMind dev" >/dev/null
"$CLI" --hash-embed ingest "TraceMind depends on Postgres and SQLite for storage" >/dev/null
"$CLI" --hash-embed ingest "Investor pitch deck references the TraceMind demo project" >/dev/null

echo "[7] scoped query inside Mercury work"
"$CLI" context use "Mercury work" >/dev/null
"$CLI" --hash-embed query "who works at Mercury" > /tmp/tm-smoke-query.out 2>&1
check "query produces a query_id" grep -qE "^Query: [0-9a-f]{8}" /tmp/tm-smoke-query.out
QID=$(grep -oE "^Query: [0-9a-f-]{36}" /tmp/tm-smoke-query.out | awk '{print $2}' || echo "")
echo "    query_id = ${QID:-<none>}"

echo
count_table() {
    sqlite3 "$TM_DATA_DIR/memory.db" \
        "SELECT count(*) FROM $1 WHERE query_id='$2'" 2>/dev/null || echo 0
}
eq_one() { [[ "$1" == "1" ]]; }

echo "[8] helpful (F-1 positive signal)"
if [[ -n "$QID" ]]; then
    "$CLI" helpful "$QID" "alice" --weight 0.4 --kind helpful >/tmp/tm-smoke-helpful.out 2>&1 || true
    POS_COUNT=$(count_table positive_signals "$QID")
    check "positive_signals row written (got $POS_COUNT)" eq_one "$POS_COUNT"
fi

echo
echo "[9] not-related (C-0.7 negative signal)"
if [[ -n "$QID" ]]; then
    "$CLI" not-related "$QID" "postgres" --kind cross_context_bridge >/tmp/tm-smoke-notrelated.out 2>&1 || true
    NEG_COUNT=$(count_table negative_signals "$QID")
    check "negative_signals row written (got $NEG_COUNT)" eq_one "$NEG_COUNT"
fi

echo
echo "[10] brief still renders"
"$CLI" brief > /tmp/tm-smoke-brief.out 2>&1
check "contradiction surfaces"     grep -q "contradictions (1)" /tmp/tm-smoke-brief.out
check "overdue bucket populated"   grep -q "overdue (3)"        /tmp/tm-smoke-brief.out
check "resolved bucket populated"  grep -q "resolved (last 7d)" /tmp/tm-smoke-brief.out

echo
echo "=== Result ==="
echo "  passed: $PASS"
echo "  failed: $FAIL"
if [[ "$FAIL" -ne 0 ]]; then exit 1; fi
