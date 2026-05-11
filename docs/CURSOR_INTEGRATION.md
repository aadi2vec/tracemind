# TraceMind + Cursor — 30-second integration

[Cursor](https://cursor.com) is an MCP-native AI code editor. TraceMind drops in as an MCP server with one config edit — no plugin, no extension, no rebuild.

**Wedge sentence:** ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.

---

## Why Cursor needs TraceMind

Cursor's built-in `.cursorrules` and indexed-codebase context are powerful for *code* — but they don't carry across projects, don't remember decisions you made over chat, don't track commitments, and don't notice when you contradict yourself.

| Property | Cursor alone | Cursor + TraceMind |
|---|---|---|
| **Persistent across sessions** | Project rules only | Yes — facts, decisions, commitments survive |
| **Cross-project memory** | No — each repo is an island | Yes — `tracemind context use …` per project |
| **Context-scoped, never blurred** | Per-repo .cursorrules; static | Bitemporal contexts; clean separation |
| **Local-only, no cloud** | Cursor sends code to its model API | TraceMind stays in `~/.tracemind/` |
| **Contradiction-aware retraction** | No | Yes — change your mind, the stale fact retracts |
| **Feedback loop** | No | Yes — `helpful` / `not_related` tune the bandit |

---

## Install (30 seconds)

### 1. Install TraceMind

```bash
curl -sSL https://tracemind.dev/install.sh | sh
```

Verify:

```bash
tracemind --version
tm-mcp --version
```

### 2. Register `tm-mcp` with Cursor

Cursor reads MCP servers from `~/.cursor/mcp.json` (user-global) or `.cursor/mcp.json` at the repo root (project-scoped). Both work the same way.

**User-global** — `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "tracemind": {
      "command": "tm-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

**Or, project-scoped** (commit it with the repo so collaborators inherit) — `.cursor/mcp.json` at the project root:

```json
{
  "mcpServers": {
    "tracemind": {
      "command": "tm-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

### 3. Tell Cursor how to use it (`.cursorrules`)

Add this stanza to your project's `.cursorrules` (or your user-global rules):

```markdown
# Memory (TraceMind)

You have access to a persistent, local memory layer via the `tracemind` MCP server.

Use it proactively:
- `memory_store` — call whenever the user shares a fact, decision, name,
  commitment, preference, or anything that might matter later. Do not
  ask permission first; ingest is cheap and reversible.
- `memory_query` — call before answering anything that references the
  past ("what did I…", "who is…", "when did…", "remind me…"). Also call
  when your working context has thinned.
- `memory_reason` — for multi-hop questions ("why did I switch from X
  to Y?").
- `memory_feedback` — when the user reacts to a recalled memory.
  `helpful` reinforces, `not_related` penalises, `cross_context_bridge`
  files a wrong-context recall.

TraceMind is contradiction-aware: when a new fact conflicts with a stored
one, the `memory_store` response surfaces the contradiction. Surface it
to the user explicitly — that retraction beat is the point.
```

### 4. Reload Cursor

`Cmd-Shift-P` → "Developer: Reload Window" (or quit and relaunch). Open the **MCP** panel in the sidebar — `tracemind` should appear with seven tools enabled.

If those show up, the integration is live.

---

## Per-project scoping (the wedge)

```bash
# create contexts once
tracemind context create "work-a"   --tags work,project-a
tracemind context create "work-b"   --tags work,project-b
tracemind context create "personal" --tags personal

# switch before opening Cursor
tracemind context use work-a
cursor ~/code/project-a
```

For sticky contexts, add this to `.cursorrules` at the repo root:

```markdown
# Context
When you start, run `tracemind context use work-a` via the terminal. All
memory operations should stay scoped to work-a unless I explicitly say
otherwise.
```

---

## The 4-beat self-demo

Identical to the Claude Code demo — same wedge, same moat, same loop. See `docs/CLAUDE_CODE_INTEGRATION.md` §"Verifying the integration end-to-end" for the full script. The only Cursor-specific note: substitute *"Open a new Cursor window"* for *"Open a new terminal"*.

1. **Persistence** — close Cursor, reopen on a different repo, ask the same question. TraceMind reconstructs.
2. **Context isolation** — `tracemind context use personal`, ask the same question. Nothing leaks.
3. **Contradiction** — change your mind. TraceMind surfaces the conflict; Cursor reports it.
4. **Feedback** — confirm or reject. The bandit updates.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| MCP panel doesn't list `tracemind` | Cursor cached prior MCP state | Reload window. If still missing, restart Cursor entirely. |
| `tm-mcp` not found | Not on PATH | `which tm-mcp`; if empty, re-run `scripts/install.sh` or use the absolute path in `mcp.json`. |
| Tools list is empty in MCP panel | Server crashed at startup | Run `tm-mcp` directly in a terminal; the JSON-RPC framing errors print on stderr. |
| Memories from project A leak into project B | Sticky context not switched | `tracemind context current`; if wrong, `tracemind context use <right-one>` and tell Cursor to retry. |

---

## What's next

- `docs/CLAUDE_CODE_INTEGRATION.md` — the canonical onboarding kit.
- `docs/GOOSE_INTEGRATION.md` — Block's MCP-native agent.
- `docs/CLINE_INTEGRATION.md` — Cline (VS Code extension) integration.
- `docs/FEEDBACK_LOOP.md` — how the bandit reinforces useful retrievals.
