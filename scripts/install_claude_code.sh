#!/usr/bin/env bash
# TraceMind ↔ Claude Code one-command integration.
#
# What this does, in order:
#   1. Verifies TraceMind is installed (`tracemind`, `tm-mcp` on PATH).
#      If not, instructs the user to run scripts/install.sh first.
#   2. Verifies Claude Code is installed (`claude` on PATH).
#      If not, links to claude.com/claude-code and exits.
#   3. Registers `tm-mcp` as a Claude Code MCP server (user scope by default,
#      `--project` to register it in the current repo via .mcp.json).
#   4. Optionally appends the recommended `# Memory` hint to a CLAUDE.md
#      (user-global at ~/.claude/CLAUDE.md by default, or ./CLAUDE.md with
#      --project). Skips if the hint is already present.
#   5. Restores the demo fixture so the retraction beat fires immediately.
#   6. Prints the 4-prompt retraction demo script for the user to paste
#      into their next `claude` session.
#
# Usage:
#   ./scripts/install_claude_code.sh             # user-global install
#   ./scripts/install_claude_code.sh --project   # register in current repo
#   ./scripts/install_claude_code.sh --no-hint   # skip CLAUDE.md edit
#   ./scripts/install_claude_code.sh --no-demo   # skip fixture restore
#
# Privacy stance: this script only edits files on your machine and runs
# local commands. It does not send anything anywhere.

set -euo pipefail

SCOPE="user"
WRITE_HINT=1
RESTORE_DEMO=1

while [ $# -gt 0 ]; do
  case "$1" in
    --project)  SCOPE="project"; shift ;;
    --no-hint)  WRITE_HINT=0; shift ;;
    --no-demo)  RESTORE_DEMO=0; shift ;;
    -h|--help)
      sed -n '2,28p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

# --- visual helpers --------------------------------------------------------

bold()  { printf '\033[1m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
red()   { printf '\033[31m%s\033[0m\n' "$*" >&2; }
dim()   { printf '\033[2m%s\033[0m\n' "$*"; }

step() {
  echo
  bold "▸ $*"
}

# --- 1. verify TraceMind ---------------------------------------------------

step "Checking TraceMind install"

if ! command -v tracemind >/dev/null 2>&1; then
  red "tracemind binary not found on PATH."
  echo  "Run scripts/install.sh first, or curl -fsSL https://tracemind.dev/install.sh | sh"
  exit 1
fi

if ! command -v tm-mcp >/dev/null 2>&1; then
  red "tm-mcp binary not found on PATH."
  echo  "Run scripts/install.sh first, or curl -fsSL https://tracemind.dev/install.sh | sh"
  exit 1
fi

TM_MCP_PATH="$(command -v tm-mcp)"
green "  tracemind: $(command -v tracemind)"
green "  tm-mcp:    $TM_MCP_PATH"

# --- 2. verify Claude Code -------------------------------------------------

step "Checking Claude Code install"

if ! command -v claude >/dev/null 2>&1; then
  red "claude binary not found on PATH."
  echo  "Install Claude Code first: https://claude.com/claude-code"
  exit 1
fi

green "  claude:    $(command -v claude)"

# --- 3. register tm-mcp with Claude Code -----------------------------------

step "Registering tm-mcp with Claude Code ($SCOPE scope)"

if [ "$SCOPE" = "project" ]; then
  # Project scope: write .mcp.json in $PWD.
  MCP_JSON="$PWD/.mcp.json"

  if [ -f "$MCP_JSON" ]; then
    if grep -q '"tracemind"' "$MCP_JSON"; then
      dim  "  $MCP_JSON already contains a tracemind entry — leaving it alone."
    else
      red "  $MCP_JSON exists but has no tracemind entry."
      echo "  Refusing to auto-merge — please add this stanza manually under mcpServers:"
      cat <<'EOF'

    "tracemind": {
      "command": "tm-mcp",
      "args": [],
      "env": {}
    }
EOF
      exit 1
    fi
  else
    cat > "$MCP_JSON" <<EOF
{
  "mcpServers": {
    "tracemind": {
      "command": "tm-mcp",
      "args": [],
      "env": {}
    }
  }
}
EOF
    green "  wrote $MCP_JSON"
  fi
else
  # User scope: prefer `claude mcp add`, fall back to instructions if it
  # is unavailable in this Claude Code version.
  if claude mcp add tracemind tm-mcp 2>/tmp/tm_mcp_add.log; then
    green "  registered via: claude mcp add tracemind tm-mcp"
  else
    dim   "  claude mcp add failed — log:"
    sed 's/^/    /' /tmp/tm_mcp_add.log
    echo  "  Add this stanza manually to ~/.claude.json under mcpServers:"
    cat <<EOF

  "tracemind": {
    "command": "$TM_MCP_PATH",
    "args": [],
    "env": {}
  }
EOF
    exit 1
  fi
fi

# --- 4. write the CLAUDE.md memory hint -----------------------------------

if [ "$WRITE_HINT" -eq 1 ]; then
  step "Adding memory hint to CLAUDE.md"

  if [ "$SCOPE" = "project" ]; then
    HINT_FILE="$PWD/CLAUDE.md"
  else
    HINT_FILE="$HOME/.claude/CLAUDE.md"
    mkdir -p "$(dirname "$HINT_FILE")"
  fi

  MARKER="<!-- tracemind:memory-hint -->"

  if [ -f "$HINT_FILE" ] && grep -q "$MARKER" "$HINT_FILE"; then
    dim "  $HINT_FILE already contains the tracemind hint — skipping."
  else
    {
      [ -f "$HINT_FILE" ] && echo
      cat <<EOF
$MARKER
# Memory

You have access to a persistent, local memory layer via the \`tracemind\` MCP server.

Use it proactively:
- \`memory_store\` — call whenever the user shares a fact, decision, name,
  commitment, preference, or anything that might matter later. Do not
  ask permission first; ingest is cheap and reversible.
- \`memory_query\` — call before answering anything that references the
  past ("what did I…", "who is…", "when did…", "remind me…"). Also call
  when your working context has thinned and the user is talking like you
  should already know something.
- \`memory_reason\` — for multi-hop questions ("why did I switch from X
  to Y?"). It reconstructs a chain across entities.
- \`memory_feedback\` — when the user reacts to a recalled memory. Use
  \`helpful\` when they confirm, \`not_related\` when they reject, and
  \`cross_context_bridge\` when the parallel you drew was wrong.

TraceMind is contradiction-aware: when a new fact conflicts with a stored
one, the \`memory_store\` response surfaces the contradiction. Surface it
to the user explicitly — that retraction beat is the point.
EOF
    } >> "$HINT_FILE"
    green "  appended hint to $HINT_FILE"
  fi
fi

# --- 5. restore the demo fixture ------------------------------------------

if [ "$RESTORE_DEMO" -eq 1 ]; then
  step "Restoring demo fixture (so the retraction beat fires immediately)"

  if tracemind demo restore --force >/dev/null 2>&1; then
    green "  fixture restored — 15 entities, 16 triples, 1 contradiction, 9 commitments"
  else
    dim "  tracemind demo restore failed — continuing without fixture."
    dim "  You can run it manually later: tracemind demo restore --force"
  fi
fi

# --- 6. print the 4-prompt retraction demo --------------------------------

step "Done. Try the 4-beat demo in a Claude Code session"

cat <<'EOF'

The wedge: persistence + context isolation (beats 1–2).
The moat:  contradiction-aware retraction (beat 3).
The loop:  feedback (beat 4).

────────────────────────────────────────────────────────────────────
BEAT 1 — Persistence across sessions (the wedge, part 1)
────────────────────────────────────────────────────────────────────
Set up a work context and start claude:

    tracemind context create "work" --tags work,demo   # one-time
    tracemind context use "work"
    cd ~/code/any-project
    claude

In the session:

    I switched the demo database from Postgres to SQLite on May 1
    because Postgres needed Docker and SQLite ships embedded. I'm
    committing to keeping it SQLite for the investor demo.

Now Ctrl-D out of claude, open a NEW terminal, cd to a DIFFERENT
directory, and run `claude` again:

    Why am I using SQLite for the demo?

→ Claude reconstructs the May 1 decision in a fresh session, in a
  different directory, with no CLAUDE.md copied. THE WEDGE.

────────────────────────────────────────────────────────────────────
BEAT 2 — Context isolation (the wedge, part 2)
────────────────────────────────────────────────────────────────────
Switch contexts and ask the same question:

    tracemind context create "personal" --tags personal   # one-time
    tracemind context use "personal"
    claude

In the session:

    What database am I using for the demo?

→ Claude correctly says it has no relevant memory in personal —
  TraceMind does NOT blur contexts. THE MISFEATURE FIX.

────────────────────────────────────────────────────────────────────
BEAT 3 — Contradiction-aware retraction (the moat)
────────────────────────────────────────────────────────────────────
Back to work:

    tracemind context use "work"
    claude

    Actually I'm changing my mind, going back to Postgres for the demo.

→ TraceMind flags the contradiction with the May 1 commitment.
  Claude surfaces it: "You committed on May 1 to keeping it SQLite.
  Are you sure you want to retract that?" THE MOAT.

────────────────────────────────────────────────────────────────────
BEAT 4 — Feedback (the compounding moat)
────────────────────────────────────────────────────────────────────
    Yes, retract the SQLite commitment. The contradiction surface
    was useful.

→ Claude calls memory_feedback {kind: "helpful"}; the bandit
  reinforces this retrieval arm. Over a week of use, TraceMind
  measurably improves on your corpus.

If anything is off, see docs/CLAUDE_CODE_INTEGRATION.md.

EOF
