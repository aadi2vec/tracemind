# TraceMind + Claude Code — 30-second integration

**Wedge sentence (W-7):** ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.

[Claude Code](https://claude.com/claude-code) is Anthropic's CLI for Claude. It's MCP-native, runs locally, and is already installed on the laptop of every developer worth pitching to. TraceMind ships an MCP server (`tm-mcp`), so the integration is a four-line config edit. **No new code on either side.**

This is the headline onboarding kit. If you only read one integration doc, read this one.

---

## Why Claude Code is the primary host

Claude Code has memory primitives (`CLAUDE.md`, slash commands, hooks, agents, plugins) — but no *persistent, context-scoped, local-only* memory layer. Every session, every directory, every `claude` invocation starts cold. `CLAUDE.md` is a static file you maintain by hand. Cross-project memory either doesn't exist or has to be copy-pasted between repos.

Drop TraceMind in and Claude Code gains four properties simultaneously — and no other memory MCP hits all four:

| Property | Without TraceMind | With TraceMind |
|---|---|---|
| **Persistent across sessions** | No — every new `claude` starts cold | Yes — facts you mention once are remembered forever |
| **Context-scoped, never blurred** | No — `CLAUDE.md` is per-repo and static | Yes — work, personal, project-A, project-B stay cleanly separated; no forced bridges |
| **Local-only, no cloud** | Yes | Yes — everything lives in `~/.tracemind/` |
| **Bitemporal + contradiction-aware** | No | Yes — when you change your mind, TraceMind retracts the stale fact and surfaces it |
| Time-travel queries ("what was I thinking on March 4?") | No | Yes — bitemporal substrate |
| Feedback loop that improves retrieval | No | Yes — `helpful` / `not_related` signals tune the bandit |

**The wedge moment a developer feels in week 1:** *"I opened `claude` in a different directory, asked about a decision I made last week, and it just knew. And when I'm in my personal context, my work stuff doesn't bleed in."*

The contradiction beat is the *architectural moat* — the answer to "what if Mem0 also goes local?" — but it is not what the user feels first. Persistence and clean context isolation are what they feel first.

---

## Install (30 seconds)

### 1. Install TraceMind

```bash
curl -sSL https://tracemind.dev/install.sh | sh
```

Installs `tracemind`, `tm-mcp`, and `tracemind-capture` to `/usr/local/bin` (or `~/.local/bin`). Creates `~/.tracemind/`. Idempotent.

Verify:

```bash
tracemind --version
```

### 2. Register `tm-mcp` with Claude Code

**One-shot CLI:**

```bash
claude mcp add tracemind tm-mcp
```

That's it. Confirm with `claude mcp list` — `tracemind` should appear.

**Or, project-scoped** (recommended for trying it on a single repo first): create `.mcp.json` at the repo root:

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

Commit it. Anyone who clones the repo and runs `claude` gets TraceMind for free.

**Or, user-global:** add to `~/.claude.json` under `mcpServers`. Available in every Claude Code session.

### 3. Tell Claude how to use it (`CLAUDE.md`)

Add this stanza to your project's `CLAUDE.md` (or `~/.claude/CLAUDE.md` for global behavior):

```markdown
# Memory

You have access to a persistent, local memory layer via the `tracemind` MCP server.

Use it proactively:
- `memory_store` — call whenever the user shares a fact, decision, name,
  commitment, preference, or anything that might matter later. Do not
  ask permission first; ingest is cheap and reversible.
- `memory_query` — call before answering anything that references the
  past ("what did I…", "who is…", "when did…", "remind me…"). Also call
  when your working context has thinned and the user is talking like you
  should already know something.
- `memory_reason` — for multi-hop questions ("why did I switch from X
  to Y?"). It reconstructs a chain across entities.
- `memory_feedback` — when the user reacts to a recalled memory. Use
  `helpful` when they confirm, `not_related` when they reject, and
  `cross_context_bridge` when the parallel you drew was wrong.

TraceMind is contradiction-aware: when a new fact conflicts with a stored
one, the `memory_store` response surfaces the contradiction. Surface it
to the user explicitly — that retraction beat is the point.
```

### 4. Restart Claude Code

```bash
# In any new terminal:
claude
```

Inside the session, ask:

> *"What memory tools do you have?"*

You should see `memory_store`, `memory_query`, `memory_reason`, `memory_analogies`, `memory_consolidate`, `memory_feedback`, `get_trace`, `list_procedures`.

If those show up, the integration is live.

---

## The four tools Claude will actually use

### `memory_store`
**When Claude calls it:** every time you share something durable — a decision, a fact, a commitment, a name, a tradeoff. Returns proactive context (top-3 related entities + 1-hop neighbours) so Claude immediately sees *"here's what I already knew."*

### `memory_query`
**When Claude calls it:** when you reference the past — *"what did I decide about X?", "who is Y?", "when did I last…"* — or when its working context has thinned.

### `memory_reason`
**When Claude calls it:** for multi-hop questions — *"why did I switch from X to Y?", "what connects A and C?"* — that need chain reconstruction across entities.

### `memory_feedback`
**When Claude calls it:** when you react to a recalled memory. Three kinds:
- `helpful` → reinforces the bandit arm that produced the result
- `not_related` → penalises the arm
- `cross_context_bridge` → tells TraceMind a parallel was wrong (the *misfeature* fix)

This is the loop that makes TraceMind get *better the more you use it.* No other Claude Code memory layer ships a feedback channel.

---

## Per-project scoping (the wedge)

You almost certainly have multiple projects on one machine. TraceMind's killer feature in a Claude Code setup is **per-project scoping** so memories from project A don't leak into project B's responses.

```bash
# create contexts once
tracemind context create "work-a"   --tags work,project-a
tracemind context create "work-b"   --tags work,project-b
tracemind context create "personal" --tags personal

# switch before launching Claude for a given project
tracemind context use work-a
cd ~/code/project-a
claude

# every memory_store call in this Claude session tags rows with `work-a`
# every memory_query call returns only `work-a`-scoped rows by default
```

For repos where the context should be sticky, drop this in `.mcp.json` next to the `tracemind` server stanza — or better, in a `CLAUDE.md` hint:

```markdown
# Context
When you start, call `tracemind context use work-a` (via Bash). All memory
operations should stay scoped to `work-a` unless the user explicitly says
otherwise.
```

To allow Claude to bridge contexts (rare, opt-in), the user asks:

> *"Search across all my contexts for…"*

…which TraceMind handles via `cross_context: true` on `memory_query`. Bridges that turn out to be wrong get filed with `memory_feedback {kind: "cross_context_bridge"}`, and the bandit learns to stop suggesting them.

---

## Verifying the integration end-to-end (4-minute self-demo)

The demo is built as four beats. **Beats 1–2 prove persistence + context isolation (the wedge).** Beat 3 proves contradiction-aware retraction (the moat). Beat 4 closes the feedback loop. Each beat is one user-felt win.

### Beat 1 — Persistence across sessions (the wedge, part 1)

Set up your work context and start `claude`:

```bash
tracemind context create "work" --tags work,demo   # one-time
tracemind context use "work"
cd ~/code/my-project
claude
```

In the session:

> "I switched the demo database from Postgres to SQLite on May 1 because Postgres needed Docker and SQLite ships embedded. I'm committing to keeping it SQLite for the investor demo."

→ Claude calls `memory_store`. TraceMind ingests under the `work` context.

**Now quit Claude** (`Ctrl-D`), open a *new* terminal, `cd` to a *different directory*, run `claude` again:

> "Why am I using SQLite for the demo?"

→ Claude calls `memory_query` / `memory_reason`. New session, different directory, no `CLAUDE.md` copied — and Claude reconstructs the May 1 decision with the Docker + embedded reasoning.

**This is the wedge.** The fact moved with you because it lives in `~/.tracemind/`, not in the repo.

### Beat 2 — Context isolation (the wedge, part 2)

Switch contexts and ask the same question:

```bash
tracemind context use "personal"   # create with: tracemind context create "personal"
```

Then in `claude`:

> "What database am I using for the demo?"

→ Claude calls `memory_query`, now scoped to `personal`. TraceMind returns *nothing* — the SQLite decision belongs to `work`. Claude correctly says it has no relevant memory in personal.

**This is the misfeature fix.** Most local memory systems would happily blur contexts and surface the SQLite decision anyway. TraceMind does not. Forced cross-context bridges are a failure mode, not a feature.

If you genuinely want to bridge contexts (rare, opt-in):

> "Search across all my contexts for the database decision."

→ Claude calls `memory_query {cross_context: true}`. The result surfaces with a `cross-context` tag. If the bridge was wrong, the user files `memory_feedback {kind: "cross_context_bridge"}` and the bandit learns to stop suggesting it.

### Beat 3 — Contradiction-aware retraction (the moat)

Back to work context:

```bash
tracemind context use "work"
claude
```

> "Actually I'm changing my mind, going back to Postgres for the demo."

→ Claude calls `memory_store`. TraceMind detects the contradiction with the May 1 decision and surfaces it. Claude says: *"You committed on May 1 to keeping it SQLite for the investor demo because Postgres needed Docker. Are you sure you want to retract that?"*

**That sentence is the moat.** No other memory layer for any MCP host produces it — none of them ship a TMS.

### Beat 4 — Feedback (the compounding moat)

> "Yes, retract the SQLite commitment. The contradiction surface was useful."

→ Claude calls `memory_feedback {kind: "helpful"}`. The bandit reinforces the arm that surfaced the contradiction. Over a week of use, TraceMind measurably improves on your corpus — the F-2..F-5 self-improvement loop running in the background.

**This is the compounding moat.** Cloud competitors cannot copy this without uploading your corrections.

---

If all four beats fire, the integration is fully wired. **Beats 1–2 are what makes a user say "I won't go back."** Beats 3–4 are what makes the architecture defensible against the next memory startup that copies the local-only stance.

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| Tools don't appear in Claude Code | `which tm-mcp` — make sure the binary is on the PATH Claude sees. `claude mcp list` should show `tracemind`. |
| `tm-mcp` exits immediately | Run `tm-mcp` standalone in a terminal; it should sit idle waiting for stdin. If it exits with an error, check `~/.tracemind/` permissions. |
| Empty queries on a fresh install | Run `tracemind status` — confirms bandit state + entity count. New install has zero memories until you ingest. |
| Wrong context returned | Run `tracemind context current` — confirm the active context matches what you expect. |
| Claude doesn't call memory tools proactively | Strengthen the `CLAUDE.md` stanza above. Some Claude models need explicit "call this" language. |
| Want to start clean | `rm -rf ~/.tracemind/` (destructive) or `tracemind demo restore --force` for a pre-seeded fixture. |

---

## What we want from design partners

If you're running this, we want three things:

1. **A weekly 15-minute call.** What did you ask Claude this week? What did TraceMind get wrong? What would make you uninstall it?
2. **`tracemind share-usage --to founders@tracemind.dev`** — opt-in, prints a local JSON snapshot (no telemetry) of daily-active flags + feedback signal counts. Paste it back to us.
3. **One 60-second video** at the 4-week mark — your face, your voice, one sentence: *"with TraceMind, Claude Code now [does X], and I won't go back to [Y]."* Or, equally valuable: *"I uninstalled because [Z]."*

That feedback is more valuable to us than any benchmark.

---

## Why this matters strategically (founder note)

Claude Code is the right primary host because every venture investor we'd pitch already has it installed. The demo runs in the meeting. The partner pastes the four prompts above into their own `claude` session and feels the retraction beat live.

Goose is our second integration (`docs/GOOSE_INTEGRATION.md`) — it proves TraceMind is host-agnostic and local-first, which pre-empts the *"what if Anthropic builds this"* objection. Tomorrow we ship into Cline, Cursor, ChatGPT Desktop, and any other MCP host. The architecture doesn't care.

The one-sentence pitch:

> *TraceMind is persistent memory for every AI you use — scoped to the right context, never blurred, never uploaded. One memory across Claude Code, Goose, Cline, Cursor, and the Tauri flagship; context-segmented by default so work doesn't bleed into personal; entirely on your machine. The contradiction beat — the moment your agent says "you told me you weren't doing X anymore" — is the architectural moat that makes the wedge defensible.*

---

## Status (2026-05-11)

- `tm-mcp` ships MCP protocol `2024-11-05` over stdio JSON-RPC — Claude Code-native.
- All 8 tools above are wired and tested.
- Context scoping (Sprint C-0) is fully landed.
- Feedback loop (F-1) is live: `helpful` / `not_related` / `cross_context_bridge` all flow into the bandit.
- Tier-1 LLM (Qwen 2.5 1.5B Q4) is scaffolded — auto-default coming in Q-1 (P2 in `TASKS.md`). Until then, Tier-0 extractive answers are the fallback.
