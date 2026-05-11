# TraceMind + Goose — 60-second integration

**Wedge sentence (W-7):** ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.

[Goose](https://github.com/block/goose) is Block's open-source local AI agent. It's MCP-native: every tool, every memory layer, every integration is an MCP server. TraceMind already ships one (`tm-mcp`), so the integration is pure configuration — **no new code on either side**.

This doc is the onboarding kit for design partners who already use Goose.

---

## Why plug TraceMind into Goose

Goose is excellent at *acting*. It is, by design, *amnesiac* — every session starts cold. The agent doesn't remember what you decided last week, doesn't keep your work and personal contexts cleanly separated, and doesn't notice when you contradict yourself a month later.

TraceMind drops in as the persistent, context-scoped, local-only memory layer behind it. Four properties land simultaneously — no other memory MCP hits all four:

| Property | Without TraceMind | With TraceMind |
|---|---|---|
| **Persistent across sessions** | No — every new session is cold | Yes — facts you mention once are remembered forever |
| **Context-scoped, never blurred** | No | Yes — work, personal, project-A stay cleanly separated; no forced cross-context bridges |
| **Local-only, no cloud** | Yes | Yes — everything lives in `~/.tracemind/` |
| **Bitemporal + contradiction-aware** | No | Yes — when reality changes, TraceMind retracts the stale fact and surfaces it |
| Time-travel queries ("what was I thinking on March 4?") | No | Yes — bitemporal substrate |
| Feedback loop that improves retrieval | No | Yes — `helpful` / `not_related` / `cross_context_bridge` tune the bandit |

**The wedge moment a user feels in week 1:** *"I started Goose in a different project, asked about a decision I made last week, and it just knew. And when I'm in my personal context, my work stuff doesn't bleed in."*

The contradiction beat is the *architectural moat* — the answer to "what if Mem0 also goes local?" — but it is not what the user feels first. Persistence and clean context isolation are.

---

## Install (one minute)

### 1. Install TraceMind

```bash
curl -sSL https://tracemind.dev/install.sh | sh
```

Installs `tracemind`, `tm-mcp`, and `tracemind-capture` to `/usr/local/bin` (or `~/.local/bin`). Creates `~/.tracemind/`. Idempotent.

Verify:

```bash
tracemind --version
tm-mcp --help 2>&1 | head -1   # should print MCP server banner on stderr
```

### 2. Register `tm-mcp` as a Goose extension

Open `~/.config/goose/config.yaml` and add (or merge into) the `extensions:` block:

```yaml
extensions:
  tracemind:
    type: stdio
    enabled: true
    cmd: tm-mcp
    args: []
    envs: {}
    timeout: 30
```

Or interactively:

```bash
goose configure
# → Add Extension → Command-line Extension
# Name: tracemind
# Command: tm-mcp
```

### 3. Restart Goose

```bash
goose session
```

Inside the session, ask:

> *"What memory tools do you have?"*

You should see `memory_store`, `memory_query`, `memory_reason`, `memory_analogies`, `memory_consolidate`, `memory_feedback`, `get_trace`, `list_procedures` in the response.

If those show up, the integration is live.

---

## The four tools Goose will actually use

Goose's LLM (Claude, GPT-4, or local) will pick tools based on the descriptions. The most-used four:

### `memory_store`
**When Goose calls it:** every time the user shares something durable — a decision, a fact, a commitment, a name. Returns proactive context (top-3 related entities + 1-hop neighbours) so Goose immediately sees *"here's what I already knew."*

### `memory_query`
**When Goose calls it:** when the user references the past — *"what did I decide about X?", "who was Y again?", "when did I last…"* — or when Goose's working context has thinned.

### `memory_reason`
**When Goose calls it:** for multi-hop questions — *"why did I switch from X to Y?", "what connects A and C?"* — that need chain reconstruction across entities.

### `memory_feedback`
**When Goose calls it:** when the user reacts to a recalled memory. Three kinds:
- `helpful` → reinforces the bandit arm that produced the result
- `not_related` → penalises the arm
- `cross_context_bridge` → tells TraceMind a parallel was wrong (the *misfeature* fix)

This is the loop that makes TraceMind get *better the more the user uses it.* No other memory MCP ships a feedback channel.

---

## Recommended Goose hint (`.goosehints`)

Goose reads `~/.config/goose/.goosehints` as a system-prompt prelude. Add this so Goose actually *uses* TraceMind aggressively:

```
You have access to a persistent memory layer via the `tracemind` extension.

Use it proactively:
- Call `memory_store` whenever the user shares a fact, decision, name,
  commitment, or preference that might matter later. Do not ask first.
- Call `memory_query` before answering any question that references the
  past ("what did I…", "who was…", "when did…", "remind me about…").
- Call `memory_reason` for multi-hop questions where you need to trace
  a chain across multiple entities.
- When the user reacts to a recalled memory ("yes that's right", "no,
  that's wrong", "wrong context"), call `memory_feedback` with the
  matching kind.

The memory is local to this machine and contradiction-aware. If a stored
fact conflicts with a new one, TraceMind will flag it — surface that
contradiction to the user explicitly.
```

---

## Scoping by context (the wedge)

Most users keep multiple projects on one machine. TraceMind's killer feature in a Goose setup is **per-project scoping** so memories from project A don't leak into project B's responses.

```bash
# create contexts once
tracemind context create "work"     --tags work,company
tracemind context create "personal" --tags personal

# switch before launching Goose for a given project
tracemind context use work
goose session

# every memory_store call in this Goose session tags rows with `work`
# every memory_query call returns only `work`-scoped rows by default
```

To allow Goose to bridge contexts (rare, opt-in), the user can ask:

> *"Search across all my contexts for…"*

…which TraceMind handles via `cross_context: true` on `memory_query`. Bridges that turn out to be wrong get filed with `memory_feedback {kind: "cross_context_bridge"}`, and the bandit learns to stop suggesting them.

---

## Verifying the integration end-to-end

Inside a `goose session`:

```
> Remember that I switched from Postgres to SQLite for the demo on May 1.
[Goose calls memory_store, returns: "Got it. I also noticed you previously mentioned…"]

> Why did I make that switch?
[Goose calls memory_query → memory_reason, reconstructs the decision chain]

> Actually I changed my mind, going back to Postgres.
[Goose calls memory_store; TraceMind flags the contradiction with the May 1 entry]

> I told Goose that wasn't relevant.
[Goose calls memory_feedback {kind: "not_related"}; bandit penalises that arm]
```

If all four beats work, the integration is fully wired.

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| Tools don't appear in Goose | `which tm-mcp` — make sure the binary is on the PATH Goose sees. Restart Goose after editing `config.yaml`. |
| `tm-mcp` exits immediately | Run `tm-mcp` standalone in a terminal; it should sit idle waiting for stdin. If it exits with an error, check `~/.tracemind/` permissions. |
| Empty queries | Run `tracemind status` — confirms bandit state + entity count. New install has zero memories until you ingest. |
| Wrong context returned | Run `tracemind context current` — confirm the active context matches what you expect. |
| Want to start clean | `rm -rf ~/.tracemind/` (destructive) or `tracemind demo restore --force` for a pre-seeded fixture. |

---

## What we want from design partners

If you're using this integration, we want three things from you:

1. **A weekly 15-minute call.** What did you ask Goose this week? What did TraceMind get wrong? What would make you uninstall it?
2. **`tracemind share-usage --to founders@tracemind.dev`** — opt-in, prints a local JSON snapshot (no telemetry) of daily-active flags + feedback signal counts. Paste it back to us.
3. **One 60-second video** at the 4-week mark — your face, your voice, one sentence: *"with TraceMind, Goose now [does X], and I won't go back to [Y]."* Or, equally valuable: *"I uninstalled it because [Z]."*

That feedback is more valuable to us than any benchmark.

---

## Status (2026-05-11)

- `tm-mcp` ships MCP protocol `2024-11-05` over stdio JSON-RPC — Goose-native.
- All 8 tools above are wired and tested.
- Context scoping (C-0) is fully landed: every ingest is tagged, every query is scoped.
- Feedback loop (F-1) is live: `helpful` / `not_related` / `cross_context_bridge` all flow back into the bandit.
- Tier-1 LLM (Qwen 2.5 1.5B Q4) is scaffolded — auto-default coming in Q-1 (P2 in `TASKS.md`). Until then, Tier-0 extractive answers are the fallback.
