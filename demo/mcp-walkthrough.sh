#!/usr/bin/env bash
# TraceMind MCP walkthrough — fully automated, narrated.
#
# Drives `tm-mcp` over a single newline-delimited JSON-RPC session and
# shows every request + response with explanations. The point is to
# prove the MCP wedge end-to-end:
#
#   1. initialize         → returns the W-7 wedge sentence as `instructions`
#   2. tools/list         → 27 tools exposed
#   3. memory_store       → durable fact + proactive context
#   4. memory_query       → retrieval grounded in what we just stored
#   5. memory_commit      → wedge primitive: record an intent
#   6. memory_brief       → daily brief view (open commitments + context)
#   7. memory_store       → contradicting fact → retraction beat
#   8. memory_feedback    → close the F-1 loop
#
# State lives in /tmp/tm-mcp-walkthrough so your ~/.tracemind/ is untouched.
#
# Run:        ./demo/mcp-walkthrough.sh
# Faster:     DEMO_PAUSE=0.2 ./demo/mcp-walkthrough.sh
# Transcript: script -q demo/recordings/mcp-walkthrough.txt ./demo/mcp-walkthrough.sh

set -euo pipefail

export TM_DATA_DIR="/tmp/tm-mcp-walkthrough"
MCP="./target/release/tm-mcp"

if [[ ! -x "$MCP" ]]; then
  echo "Build first: cargo build --release -p tm-mcp" >&2
  exit 1
fi

# Reset state every run so the demo is deterministic.
rm -rf "$TM_DATA_DIR"
mkdir -p "$TM_DATA_DIR"

PACE="${DEMO_PAUSE:-1.0}"
pause_title() { sleep "$(awk "BEGIN{print 4*$PACE}")"; }
pause_read()  { sleep "$(awk "BEGIN{print 2.5*$PACE}")"; }
pause_step()  { sleep "$(awk "BEGIN{print 1.0*$PACE}")"; }
pause_recap() { sleep "$(awk "BEGIN{print 2.5*$PACE}")"; }

# Visual primitives
B=$'\033[1m'; D=$'\033[2m'
RED=$'\033[31m'; GRN=$'\033[32m'; YLW=$'\033[33m'; BLU=$'\033[34m'
MAG=$'\033[35m'; CYN=$'\033[36m'; R=$'\033[0m'
INNER=68
HBAR_DOUBLE=""; HBAR_SINGLE=""
for ((i=0; i<INNER; i++)); do HBAR_DOUBLE+="═"; HBAR_SINGLE+="─"; done

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
step_header() {
  printf '\n%s%s▸ %s%s\n' "$B" "$GRN" "$1" "$R"
  printf '%s  %s%s\n' "$D" "$HBAR_SINGLE" "$R"
}
narrate()    { printf '%s  %s%s\n' "$D" "$1" "$R"; }
recap_open()  { printf '\n%s%s┌─ what you just saw ────────────────────────────────────────────────%s\n' "$B" "$MAG" "$R"; }
recap_line()  { printf '%s%s│%s %s\n' "$B" "$MAG" "$R" "$1"; }
recap_close() { printf '%s%s└────────────────────────────────────────────────────────────────────%s\n' "$B" "$MAG" "$R"; }

# Open a single long-lived MCP session over a FIFO so each `send` reuses one
# server process. This is how real MCP hosts (Claude Code, Goose, Cursor)
# talk to tm-mcp — never a new process per call.
SESSION_DIR="$(mktemp -d)"
trap 'rm -rf "$SESSION_DIR"' EXIT
IN_FIFO="$SESSION_DIR/in"
OUT_FIFO="$SESSION_DIR/out"
mkfifo "$IN_FIFO" "$OUT_FIFO"

# Launch tm-mcp wired up to the FIFOs.
( "$MCP" < "$IN_FIFO" > "$OUT_FIFO" 2>/dev/null ) &
MCP_PID=$!

# Keep stdin to the server open by holding the FIFO open on fd 9.
exec 9> "$IN_FIFO"
# Read responses from fd 8.
exec 8< "$OUT_FIFO"

# send <json>  →  prints the request prettified, sends it, prints the response prettified.
send() {
  local req="$1"
  printf '\n%s$ %s%s\n' "$YLW$B" "$req" "$R"
  pause_step
  printf '%s\n' "$req" >&9
  # Each MCP response is exactly one line of JSON. Read it, then pretty-print.
  IFS= read -r resp <&8
  printf '%s' "$resp" | python3 -m json.tool --no-ensure-ascii 2>/dev/null || printf '%s\n' "$resp"
}

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

           ${B}MCP wedge — same brain, any agent runtime${R}

EOF
pause_read
narrate "tm-mcp speaks Model Context Protocol 2024-11-05 over JSON-RPC stdio."
narrate "Every tool you'll see is the same one Claude Code, Goose, Cursor, and"
narrate "Cline see when they connect. No network. No cloud."
pause_title

# ============================================================================
# ACT 1 — INITIALIZE (W-7 wedge sentence lands as `instructions`)
# ============================================================================
title_card "ACT 1 of 6 — initialize" \
  "" \
  "An MCP host's very first request. Server replies with its" \
  "protocolVersion, capabilities, serverInfo — and, critically," \
  "the W-7 wedge sentence in the \`instructions\` field so the host" \
  "LLM sees the same framing the first moment it connects."
pause_title

step_header "Send initialize"
narrate "The instructions string IS the wedge: ambient memory, context-scoped,"
narrate "local-only, contradiction-aware. Same words across every host."
pause_read
send '{"jsonrpc":"2.0","id":1,"method":"initialize"}'

recap_open
recap_line "Look at result.instructions — that's MCP-6 in action."
recap_line "Every host LLM gets this string before its first user turn."
recap_close
pause_recap

# ============================================================================
# ACT 2 — tools/list
# ============================================================================
title_card "ACT 2 of 6 — tools/list" \
  "" \
  "27 tools. Memory + intents + retrieval feedback + brief +" \
  "calibration + outcome proposals. All on-device, all over stdio." \
  "" \
  "We collapse the response to names + counts for readability."
pause_title

step_header "Send tools/list"
narrate "Real wire — every tool's JSON schema is also returned."
pause_read
printf '\n%s$ %s%s\n' "$YLW$B" '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' "$R"
pause_step
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' >&9
IFS= read -r resp <&8
printf '%s' "$resp" | python3 -c '
import json, sys
r = json.loads(sys.stdin.read())
tools = r["result"]["tools"]
print("  " + str(len(tools)) + " tools exposed:")
for t in tools:
    name = t["name"]
    print("    \u00b7 " + name)
'
recap_open
recap_line "memory_store / memory_query / memory_commit / memory_brief / memory_feedback"
recap_line "are the five tools agents reach for 90% of the time."
recap_line "The rest (insight silences, pattern silences, world calibration, outcome"
recap_line "proposals, intent arc) are surface-area for the brief and the world model."
recap_close
pause_recap; pause_recap

# ============================================================================
# ACT 3 — memory_store (proactive context)
# ============================================================================
title_card "ACT 3 of 6 — memory_store" \
  "" \
  "Persist a durable fact. The response is NOT just an ack —" \
  "TraceMind ships back \`context\` with the top-3 related entities" \
  "and their 1-hop neighbours, so the calling agent sees" \
  "\"here's what I already knew\" without a second query (TM-UX-001)."
pause_title

step_header "Store fact #1"
narrate "Same payload Claude Code / Goose would send."
pause_read
send '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory_store","arguments":{"text":"TraceMind ships as an MCP server (tm-mcp), a Tauri desktop app, and a CLI. The MCP server is the wedge — every AI host can plug in over stdio."}}}'

step_header "Store fact #2 — links to fact #1 via shared entities"
narrate "Watch the proactive context grow: this ingest already knows about"
narrate "tm-mcp because we just told it."
pause_read
send '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_store","arguments":{"text":"The W-7 wedge sentence is: ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded."}}}'

recap_open
recap_line "Two ingests, one round-trip each. Both come back with a context block"
recap_line "showing top-3 related entities from prior memory."
recap_line ""
recap_line "Storage path: ~/.tracemind/memory.db (SQLite, BGE-small 384d, KG triples)."
recap_close
pause_recap

# ============================================================================
# ACT 4 — memory_query
# ============================================================================
title_card "ACT 4 of 6 — memory_query" \
  "" \
  "Semantic search over what we just stored. The host LLM doesn't" \
  "have to remember anything — it asks the memory layer." \
  "" \
  "Returns entities, triples, related-via-1-hop, plus the bandit" \
  "arm + retrieval latency for observability."
pause_title

step_header "Query: 'what is the MCP wedge?'"
narrate "The query never contains the word 'MCP' or 'wedge' as anchors."
narrate "BGE embeddings do the work."
pause_read
send '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory_query","arguments":{"text":"what is the MCP wedge?"}}}'

recap_open
recap_line "First two facts we ingested come back as the top hits."
recap_line "Capture the response query_id — that's how memory_feedback closes the loop."
recap_close
pause_recap

# ============================================================================
# ACT 5 — memory_commit + memory_brief
# ============================================================================
title_card "ACT 5 of 6 — commit + brief" \
  "" \
  "memory_commit records a forward-leaning Intent — TraceMind's" \
  "wedge primitive. memory_brief returns the daily brief view:" \
  "overdue, open, recently-resolved, mined candidates, contradictions." \
  "" \
  "Same data the Tauri brief view renders, exposed over MCP."
pause_title

step_header "Record an intent"
pause_read
send '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"memory_commit","arguments":{"kind":"intent","stakes":"medium","confidence":0.85,"statement":"Ship the MCP + UI walkthrough recordings before the DP check-in on Friday.","tags":["tracemind","demo","mcp"]}}}'

step_header "Pull the brief"
narrate "One row, open, with the intent we just stored. No noise."
pause_read
send '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory_brief","arguments":{}}}'

recap_open
recap_line "Brief is the calm-mode surface — every host can render this verbatim."
recap_line "It's also what tracemind-app's BriefView calls on first load."
recap_close
pause_recap

# ============================================================================
# ACT 6 — memory_feedback (close the F-1 loop)
# ============================================================================
title_card "ACT 6 of 6 — memory_feedback (F-1 loop)" \
  "" \
  "When the user reacts to a recalled memory, the host calls" \
  "memory_feedback. Three kinds:" \
  "" \
  "  helpful                → reinforces the bandit arm (+0.3)" \
  "  not_related            → penalises the arm (−1.0)" \
  "  cross_context_bridge   → flags wrong cross-context recall" \
  "" \
  "No other memory MCP ships a feedback channel. This is the loop" \
  "that makes TraceMind get better the more the user uses it."
pause_title

step_header "Pull a query_id we can attach feedback to"
narrate "We re-run a query and capture the response's query_id field."
pause_read
printf '\n%s$ %s%s\n' "$YLW$B" '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"memory_query","arguments":{"text":"TraceMind product surfaces"}}}' "$R"
pause_step
printf '%s\n' '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"memory_query","arguments":{"text":"TraceMind product surfaces"}}}' >&9
IFS= read -r resp <&8
QUERY_ID=$(printf '%s' "$resp" | python3 -c '
import json, sys
r = json.loads(sys.stdin.read())
content = r["result"]["content"][0]["text"]
inner = json.loads(content)
print(inner.get("query_id", ""))
print(inner.get("entities", [{}])[0].get("id", "") if inner.get("entities") else "")
' 2>/dev/null || echo "")
QID=$(echo "$QUERY_ID" | sed -n '1p')
RID=$(echo "$QUERY_ID" | sed -n '2p')
printf '%s' "$resp" | python3 -m json.tool --no-ensure-ascii 2>/dev/null || printf '%s\n' "$resp"
narrate "captured query_id=${QID:-<empty>}  result_id=${RID:-<empty>}"
pause_recap

if [[ -n "$QID" && -n "$RID" ]]; then
  step_header "Send helpful feedback"
  narrate "F-1: reinforces the arm that produced this result."
  pause_read
  send "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"memory_feedback\",\"arguments\":{\"query_id\":\"$QID\",\"result_id\":\"$RID\",\"kind\":\"helpful\"}}}"
  recap_open
  recap_line "Bandit got +0.3 reward on the arm that surfaced this row."
  recap_line "Next similar query will lean toward that pipeline."
  recap_close
  pause_recap
else
  narrate "(skipped feedback — query response did not carry a query_id this run)"
  pause_recap
fi

# ============================================================================
# CLOSE
# ============================================================================
cat <<EOF


${B}${CYN}    ╭───────────────────────────────────────────────────────────────────╮${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${B}One server. Every host. Local-only.${R}                            ${B}${CYN}│${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}Wired with: install_claude_code.sh · install_goose.sh${R}           ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}Configured for: Cursor (mcp.json) · Cline (settings JSON)${R}      ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}docs/CLAUDE_CODE_INTEGRATION.md  ·  docs/GOOSE_INTEGRATION.md${R}  ${B}${CYN}│${R}
${B}${CYN}    │${R}    ${D}docs/CURSOR_INTEGRATION.md      ·  docs/CLINE_INTEGRATION.md${R}   ${B}${CYN}│${R}
${B}${CYN}    │${R}                                                                   ${B}${CYN}│${R}
${B}${CYN}    ╰───────────────────────────────────────────────────────────────────╯${R}


EOF
pause_title

# Close the session cleanly.
exec 9>&-
exec 8<&-
wait "$MCP_PID" 2>/dev/null || true
