#!/usr/bin/env bash
# TraceMind UI walkthrough — seeds realistic state, launches tracemind-app,
# prints narration cards in the terminal so a presenter knows what to click.
#
# State lives in /tmp/tm-ui-walkthrough so your real ~/.tracemind/ is untouched.
#
# Run:   ./demo/ui-walkthrough.sh
# Record: ./demo/record-ui.sh
#
# View tour (paced for a 90-second screen recording):
#   1. Onboarding      — first-run gate (no usage history → routed here)
#   2. Brief           — read-dot, dismiss, archive, active-context label
#   3. Query           — context-switch banner (UI-14)
#   4. Commitments     — timeline view (UI-10)
#   5. Calibration     — gauge view (UI-11)
#   6. Settings        — capture permissions + usage stats (UI-12/13)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export TM_DATA_DIR="/tmp/tm-ui-walkthrough"
TM="$REPO_ROOT/target/release/tracemind"
TAURI="$REPO_ROOT/target/release/tracemind-app"

if [[ ! -x "$TM" || ! -x "$TAURI" ]]; then
  echo "Build first:" >&2
  echo "  cargo build --release -p tm-cli" >&2
  echo "  cargo build --release -p tm-tauri --features custom-protocol" >&2
  exit 1
fi

# Fresh state every run.
rm -rf "$TM_DATA_DIR"
mkdir -p "$TM_DATA_DIR"

PACE="${DEMO_PAUSE:-1.0}"
pause_title() { sleep "$(awk "BEGIN{print 5*$PACE}")"; }
pause_read()  { sleep "$(awk "BEGIN{print 3*$PACE}")"; }
pause_step()  { sleep "$(awk "BEGIN{print 2*$PACE}")"; }
pause_long()  { sleep "$(awk "BEGIN{print 8*$PACE}")"; }

B=$'\033[1m'; D=$'\033[2m'
GRN=$'\033[32m'; YLW=$'\033[33m'; CYN=$'\033[36m'; MAG=$'\033[35m'; R=$'\033[0m'
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
step()    { printf '\n%s%s▸ %s%s\n' "$B" "$GRN" "$1" "$R"; printf '%s  %s%s\n' "$D" "$HBAR_SINGLE" "$R"; }
do_now()  { printf '%s%s▶ DO NOW:%s %s\n' "$B" "$YLW" "$R" "$1"; }
narrate() { printf '%s  %s%s\n' "$D" "$1" "$R"; }

# ============================================================================
# SEED — realistic state so every view has something to render
# ============================================================================

seed() {
  # Context segmentation (C-0)
  "$TM" context create "work" --tags work,tracemind >/dev/null 2>&1 || true
  "$TM" context create "personal" --tags personal >/dev/null 2>&1 || true
  "$TM" context use work >/dev/null 2>&1

  # A handful of ingests so query / brief / graph have data.
  "$TM" ingest "TraceMind ships as an MCP server, a Tauri desktop app, and a CLI binary." >/dev/null
  "$TM" ingest "The W-7 wedge sentence: ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded." >/dev/null
  "$TM" ingest "The Tauri app's BriefView lets you mark rows read, dismiss them, or archive them — all state persists across restarts via localStorage." >/dev/null
  "$TM" ingest "QueryView surfaces a context-switch banner when the query smells like a different context, with one-click 'switch and re-run'." >/dev/null

  # A few commitments — open, resolved, with a mix of polarities.
  local IDS=()
  IDS+=("$("$TM" commit --kind intent   --stakes medium --confidence 0.85 --tags 'tracemind,demo,mcp'    --no-preflight 'Ship the MCP + UI walkthrough recordings before the DP check-in on Friday' 2>&1 | awk '/^commitment/ {print $2}')")
  IDS+=("$("$TM" commit --kind decision --stakes low    --confidence 0.9  --tags 'tracemind,arch'        --no-preflight 'Use BGE-small-en-v1.5 as the default embedder' 2>&1 | awk '/^commitment/ {print $2}')")
  IDS+=("$("$TM" commit --kind intent   --stakes high   --confidence 0.7  --tags 'tracemind,investor'    --no-preflight 'Send the new investor deck to first three reviewers this week' 2>&1 | awk '/^commitment/ {print $2}')")
  IDS+=("$("$TM" commit --kind intent   --stakes medium --confidence 0.8  --tags 'tracemind,docs'        --no-preflight 'Write CURSOR_INTEGRATION.md + CLINE_INTEGRATION.md before pitching new hosts' 2>&1 | awk '/^commitment/ {print $2}')")
  IDS+=("$("$TM" commit --kind hypothesis --stakes low  --confidence 0.6  --tags 'tracemind,nlp'         --no-preflight 'GLiNER picks up TM-INTENT-### codes as Products consistently' 2>&1 | awk '/^commitment/ {print $2}')")

  # Resolve a couple so the brief has a "recently resolved" section
  # and so calibration has out-of-sample rows.
  "$TM" resolve --polarity as_expected "${IDS[1]}" "Adopted BGE-small everywhere; LoCoMo lift confirmed" >/dev/null
  "$TM" resolve --polarity as_expected "${IDS[3]}" "Both integration docs landed in PR #40"            >/dev/null
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

           ${B}Tauri desktop — UI-8 through UI-14 walkthrough${R}

EOF
pause_read
narrate "We'll seed a fresh ~/.tracemind/, launch tracemind-app pointed at it,"
narrate "and click through the six new surfaces. ~3 minutes."
pause_title

# ============================================================================
# SEED + LAUNCH
# ============================================================================
title_card "Seeding fresh state" \
  "" \
  "  TM_DATA_DIR=$TM_DATA_DIR" \
  "" \
  "  · 2 contexts (work + personal, work active)" \
  "  · 4 ingested memories" \
  "  · 5 commitments (3 open, 2 resolved)"
pause_title

step "Seeding…"
seed
narrate "✓ seeded."
pause_step

step "Launching tracemind-app"
narrate "The app opens against TM_DATA_DIR=$TM_DATA_DIR."
do_now "Wait for the Tauri window to appear (3–5 s on first launch)."
pause_step
"$TAURI" >/dev/null 2>&1 &
TAURI_PID=$!
pause_long

# ============================================================================
# VIEW 1 — Onboarding (UI-8)
# ============================================================================
title_card "VIEW 1 — Onboarding (UI-8)" \
  "" \
  "First-run users see this instead of the brief. The app calls" \
  "getUsageStats(); if first_seen is null AND total_queries == 0," \
  "App.tsx routes here. The sidebar is hidden — focused start." \
  "" \
  "Sets the W-7 framing in the user's head before they see anything else."
pause_title
do_now "If onboarding shows, read it. Click 'Get started' / 'Skip' to enter the app."
pause_long

# ============================================================================
# VIEW 2 — Brief (UI-9)
# ============================================================================
title_card "VIEW 2 — Brief (UI-9)" \
  "" \
  "Calm-mode home. Shows overdue / open / recently-resolved" \
  "commitments plus contradictions if any." \
  "" \
  "What's new (UI-9):" \
  "  · scope label in header (the active context: 'work')" \
  "  · read-dot indicator on each row (localStorage)" \
  "  · hover row → reveal ✕ (dismiss) + ⌫ (archive)" \
  "  · archived section is collapsible with 'restore'"
pause_title
do_now "Click 'Brief' in the sidebar (or just observe — it's the default)."
pause_read
do_now "Hover any open commitment row — note the hidden ✕ + ⌫ controls."
pause_read
do_now "Click the read-dot next to a row to mark it read; dot dims."
pause_read
do_now "Click ⌫ on a row to archive it; expand 'Archived' to restore."
pause_long

# ============================================================================
# VIEW 3 — Query + context banner (UI-14)
# ============================================================================
title_card "VIEW 3 — Query + context banner (UI-14)" \
  "" \
  "Type a query that smells like a DIFFERENT context. The view" \
  "fires suggestContext() in parallel with the query itself; if" \
  "TraceMind thinks a different context is the right scope, an" \
  "amber banner appears between the input and the results:" \
  "" \
  "    ⓘ This looks like a {personal} question. Switch & re-run?" \
  "" \
  "Click 'Switch & re-run' — useContext() flips the active scope," \
  "then the query re-runs in the new context."
pause_title
do_now "Click 'Query' in the sidebar."
pause_read
do_now "Type:  what does the wedge sentence say"
pause_read
do_now "Press Enter. Note results + the amber banner (if it triggers)."
pause_read
do_now "Click 'Switch & re-run' (or 'Dismiss' to ignore)."
pause_long

# ============================================================================
# VIEW 4 — Commitments timeline (UI-10)
# ============================================================================
title_card "VIEW 4 — Commitments timeline (UI-10)" \
  "" \
  "Every commitment as a row on a date-ordered timeline." \
  "Color-coded by kind (intent / decision / hypothesis) and by" \
  "polarity once resolved (better / as_expected / worse / mixed)." \
  "" \
  "Two of our seeded commitments are already resolved — those" \
  "show their outcome chip inline."
pause_title
do_now "Click 'Commitments' in the sidebar."
pause_read
do_now "Hover a resolved row to see the outcome description tooltip."
pause_long

# ============================================================================
# VIEW 5 — Calibration (UI-11)
# ============================================================================
title_card "VIEW 5 — Calibration (UI-11)" \
  "" \
  "Brier score + per-class reliability table for the world model." \
  "Out-of-sample only — only commitments resolved AFTER training" \
  "count, so the report is honest about generalization." \
  "" \
  "Fresh install has too few priors to populate; the panel says" \
  "'warming up' (n_evaluated=0). That's real behavior, not a bug."
pause_title
do_now "Click 'Calibration' in the sidebar."
pause_read
do_now "Read the warming-up message — explains why numbers are zero on a fresh install."
pause_long

# ============================================================================
# VIEW 6 — Settings (UI-12 + UI-13)
# ============================================================================
title_card "VIEW 6 — Settings (UI-12 + UI-13)" \
  "" \
  "Two panels:" \
  "" \
  "  Capture permissions (UI-13)" \
  "  ─ per-source toggles: clipboard / shell / browser / fs" \
  "  ─ each toggle writes ~/.tracemind/capture_permissions.json" \
  "  ─ audit row shows last grant timestamp" \
  "" \
  "  Usage stats (UI-12)" \
  "  ─ total queries / helpful feedback / negative feedback" \
  "  ─ last-seen timestamps" \
  "  ─ a 'Copy usage JSON' button for the DP check-in (DP-4)"
pause_title
do_now "Click 'Settings' in the sidebar."
pause_read
do_now "Toggle one capture source off → confirmation dialog → audit row updates."
pause_read
do_now "Scroll to the usage panel; click 'Copy usage JSON for sharing' to verify."
pause_long

# ============================================================================
# CLOSE
# ============================================================================
title_card "Done" \
  "" \
  "  · seeded ~/.tracemind/ at $TM_DATA_DIR" \
  "  · launched tracemind-app (pid $TAURI_PID)" \
  "" \
  "Quit the app (Cmd+Q) or kill the recording when you're done." \
  "Leave the data dir to inspect, or rm -rf to reset."
pause_title

narrate "Tauri app PID: $TAURI_PID — runs until you quit it."
narrate "Quit + reset: kill $TAURI_PID && rm -rf $TM_DATA_DIR"
