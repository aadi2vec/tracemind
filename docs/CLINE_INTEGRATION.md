# TraceMind + Cline — 30-second integration

[Cline](https://cline.bot) is an open-source autonomous coding agent that runs as a VS Code extension and is fully MCP-native. TraceMind drops in as one of Cline's MCP servers with a single JSON edit.

**Wedge sentence:** ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.

---

## Why Cline benefits from TraceMind

Cline can use any LLM (Anthropic, OpenAI, Gemini, local Ollama, …). It remembers within a session and within a checkpoint, but **across sessions and across projects it starts cold every time.** Its "Memory Bank" feature is a static markdown directory you maintain by hand.

| Property | Cline alone | Cline + TraceMind |
|---|---|---|
| **Persistent across sessions** | Per-task only | Yes — every fact survives |
| **Cross-project memory** | No | Yes — context per project |
| **Contradiction-aware retraction** | No | Yes |
| **Local-only, no cloud** | Depends on model provider | TraceMind is local-only regardless |
| **Feedback loop that tunes retrieval** | No | Yes |

Cline + TraceMind + a local Ollama model = a fully **offline, persistent, contradiction-aware** coding agent. That combination doesn't exist elsewhere.

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

### 2. Locate the Cline MCP settings file

Cline stores MCP configuration in a single JSON file inside VS Code's global extension storage:

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |
| Linux | `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |
| Windows | `%APPDATA%/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` |

(For VS Code Insiders, replace `Code` with `Code - Insiders`. For Cursor, replace `Code` with `Cursor` — Cline runs there too.)

Easiest: in VS Code, open the Cline sidebar → MCP Servers → **Configure MCP Servers**. VS Code opens the file directly.

### 3. Add `tracemind` to the file

```json
{
  "mcpServers": {
    "tracemind": {
      "command": "tm-mcp",
      "args": [],
      "env": {},
      "disabled": false,
      "autoApprove": [
        "memory_store",
        "memory_query",
        "memory_reason",
        "memory_analogies",
        "get_trace",
        "list_procedures"
      ]
    }
  }
}
```

`autoApprove` lets Cline call these tools without prompting on every invocation — important so the memory layer feels invisible. `memory_consolidate` and `memory_feedback` are deliberately left off the auto-approve list because they have side effects (consolidate edits the graph, feedback updates the bandit).

If the file already has other servers under `mcpServers`, just add the `tracemind` key alongside them.

### 4. Tell Cline how to use it

Cline reads `.clinerules` (or the legacy `.clineignore`-adjacent prompt files). Add this stanza to your project's `.clinerules`:

```markdown
# Memory (TraceMind)

You have access to a persistent, local memory layer via the `tracemind` MCP server.

Use it proactively:
- `memory_store` — call whenever the user shares a fact, decision, name,
  commitment, or preference. Don't ask permission; ingest is cheap.
- `memory_query` — call before answering anything that references the
  past, or whenever your working context has thinned.
- `memory_reason` — for multi-hop "why did I…" questions.
- `memory_feedback` — when the user reacts to a recalled memory.

TraceMind is contradiction-aware: when a new fact conflicts with a stored
one, the `memory_store` response surfaces the contradiction. Surface it
to the user — that retraction beat is the point.
```

For user-global behaviour, place an equivalent block in your global Cline custom instructions (Settings → Cline → Custom Instructions).

### 5. Reload the Cline panel

Open Cline → MCP Servers → you should see `tracemind` with a green dot and seven tools (`memory_store`, `memory_query`, `memory_reason`, `memory_analogies`, `memory_consolidate`, `memory_feedback`, `get_trace`, `list_procedures`).

If those show up, the integration is live.

---

## Per-project scoping (the wedge)

Same context-switching dance as the other hosts:

```bash
tracemind context create "work"     --tags work
tracemind context create "personal" --tags personal

tracemind context use work
code ~/code/project-a   # launch VS Code in that repo
```

Optional: put this in `.clinerules` so Cline scopes itself automatically:

```markdown
# Context
At the start of each task, run `tracemind context use work` via the terminal.
All memory operations should remain scoped to work unless I say otherwise.
```

---

## The 4-beat self-demo

Same beats, same wedge sentence, same moat:

1. **Persistence across sessions** — start a new Cline task in a different folder, ask about a decision from last week. TraceMind reconstructs it.
2. **Context isolation** — `tracemind context use personal`, ask the same question. Nothing leaks across.
3. **Contradiction-aware retraction** — change your mind on a stored fact. TraceMind flags the conflict; Cline surfaces it before continuing.
4. **Feedback** — confirm helpful, reject not-related. The bandit reinforces useful arms over the week.

See `docs/CLAUDE_CODE_INTEGRATION.md` §"Verifying the integration end-to-end" for the full prompt sequence.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `tracemind` not in Cline's MCP panel | Settings JSON not reloaded | Cline → MCP Servers → click the refresh icon, or reload window. |
| `tm-mcp` "command not found" in panel logs | Not on Cline's PATH | Use the absolute path in `cline_mcp_settings.json` (output of `which tm-mcp`). |
| Cline auto-prompts on every memory call | `autoApprove` array missing | Re-paste the snippet above. |
| Memories from project A leak | Sticky context wasn't switched | `tracemind context current`; switch if wrong. |
| `memory_consolidate` keeps prompting | Intentional — it has side effects | Approve once per session or add it to `autoApprove` if you want it silent. |

---

## What's next

- `docs/CLAUDE_CODE_INTEGRATION.md` — the canonical onboarding kit.
- `docs/CURSOR_INTEGRATION.md` — Cursor (MCP-native editor).
- `docs/GOOSE_INTEGRATION.md` — Block's MCP-native agent.
- `docs/FEEDBACK_LOOP.md` — bandit reinforcement under the hood.
