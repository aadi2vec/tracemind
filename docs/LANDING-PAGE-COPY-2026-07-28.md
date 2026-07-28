# TraceMind — landing page copy

**Date:** 2026-07-28
**Purpose:** the URL the YC application asks for; also the top of the design-partner funnel (validation plan W1).

Built on the wedge sentence recommended in `YC-READINESS-2026-07-28.md` §5:

> **"TraceMind is the memory that tells you when it changed its mind."**

**If you pick a different wedge, only the hero and §3 change.** Alternatives are
in §5 of the readiness memo. Rendered page: `docs/landing/index.html`.

**Honesty constraints applied throughout:** no user counts, no testimonials, no
logos, no "trusted by." Nothing here claims traction that doesn't exist. The
only social proof used is the code and the benchmark, both verifiable.

---

## Hero

**H1:** The memory that tells you when it changed its mind.

**Sub:** TraceMind is a local memory layer for the AI assistants you already
use. It remembers what you tell it, notices when new facts contradict old ones,
and says so — instead of silently overwriting. Everything runs on your laptop.

**Primary CTA:** Request early access
**Secondary CTA:** See how it works ↓

**Under the fold, small:** Free while in early access · macOS · no account, no cloud

---

## 1. The problem

**Every AI window starts from zero.**

You explain your project to Claude Code. Then again to Cursor. Then again
tomorrow. The assistants got smarter; the amnesia didn't change.

The tools that fix this want your data in their cloud — which is fine for a
demo and wrong for the thing that ends up holding your meeting notes, your
half-formed decisions, and your clipboard.

---

## 2. What TraceMind does

**One verb, every host.** Bind `memory_context_for` to a keystroke in Claude
Code, Cursor, Goose, or anything else that speaks MCP. Ask about a topic, get
back a token-budgeted, cited brief of what you already know.

**It ingests what you actually do.** Text, URLs, files, clipboard, shell
history — into a knowledge graph with an audit trail you can inspect, entry by
entry.

**Every claim is cited.** Every ingest and every query gets a UUID in an
immutable log. When it tells you something, you can see where it came from.

---

## 3. The part that's different

**Most memory systems store documents. TraceMind maintains beliefs.**

Tell it the World Cup final is in New Jersey. Later, tell it the final is in
Los Angeles. A document store keeps both and hands you whichever ranks higher.
TraceMind notices the two can't both be true, retracts the one it no longer
holds, and shows you:

```
⚠ Retraction: "final located in New Jersey"
  superseded by "final located in Los Angeles"
  → reconcile?
```

That's a truth-maintenance layer doing belief revision in the open. You see
your memory change its mind, and you get to overrule it.

**Why it matters:** you only hand a memory system your real context — the
messy, ambient, valuable stuff — once you trust it not to quietly drift. Most
systems ask for that trust. This one shows its work.

---

## 4. Local means local

Not a setting. Not "encrypted sync." There is **no network call in the request
path** — you can verify it with a packet monitor.

Everything lives in `~/.tracemind/`: one SQLite file and a JSONL audit trail.
`rm -rf` it and it's gone. That's the whole contract.

The constraint is the point. Because nothing leaves the machine, TraceMind can
watch your clipboard and shell history — inputs no cloud vendor can reasonably
ask for.

**Export anytime.** `tracemind export` writes your whole graph as
Obsidian-compatible markdown with `[[wikilinks]]`. No lock-in.

---

## 5. Under the hood

For people who want to know it's real.

| | |
|---|---|
| **Embeddings** | BGE-small-en-v1.5, 384-dim ONNX, on-device |
| **Reranking** | ColBERT (mxbai-edge-colbert), always on |
| **Store** | SQLite knowledge graph + vector index |
| **Belief revision** | JTMS truth-maintenance layer |
| **Answers** | extractive → local Qwen 2.5 1.5B → Apple Foundation Models |
| **Integration** | MCP over JSON-RPC — any MCP host |
| **Retrieval quality** | 50.32 F1 on held-out LoCoMo-style conversations |
| **Codebase** | 101,501 lines of Rust, 30 crates, 1,240 tests |

> On that F1: it's measured on conversations the retrieval policy was never
> tuned against. An earlier number of 75.49 was withdrawn after it turned out
> to have been tuned on its own test set. Details:
> [MVP-STATUS-2026-07](https://github.com/aadi2vec/tracemind/blob/main/docs/MVP-STATUS-2026-07.md).

> **Keep that callout.** Volunteering a retracted number is unusual and it is
> the most credible thing on the page. It costs nothing and buys a lot.

---

## 6. Early access

**H:** Looking for 5 people who live in Claude Code.

TraceMind is early. It works, it's tested, and almost nobody has used it yet —
which is exactly why I want a handful of people who'll tell me where it breaks.

**What you get:** the build, direct access to me, and whatever you ask for
prioritized.
**What I ask:** use it for two weeks and tell me the truth.

`[ your@email ]` → **Request access**

*Solo founder. macOS first. Windows/Linux if there's demand.*

---

## 7. FAQ

**Is this a note-taking app?**
No. There's nothing to file and no folders. It watches what you already do and
gives it back when an AI assistant needs it.

**Does it work with ChatGPT?**
Any MCP-capable host. Claude Code and Cursor are best supported today.

**What does it cost?**
Free during early access. Later a personal subscription. No plan will require
your data leaving your machine.

**How is this different from Mem0 / Zep / Letta?**
Those are memory layers sold to developers building AI products — cloud APIs in
the serving path. TraceMind is for the person *using* AI assistants, running
on their own machine. Closest in spirit is Zep's temporal knowledge graph;
the difference is that TraceMind surfaces belief revision to *you* as an event
you can act on, rather than resolving it silently inside retrieval.

**Is it open source?**
Not yet. The audit trail and export exist so you're never locked in regardless.

**Who's building it?**
One person, since March 2026.

---

## Meta

- **Title:** TraceMind — local AI memory that shows its work
- **Description:** A local memory layer for Claude Code, Cursor, and any MCP host. Remembers what you tell it, tells you when new facts contradict old ones. Runs entirely on your laptop.
- **OG image:** the retraction beat terminal output — it's the whole pitch in one screenshot.

## Before publishing

- [ ] Confirm the wedge sentence (readiness memo §5) — everything keys off it
- [ ] Working email capture (Formspree/Buttondown is fine; don't build a backend)
- [ ] Replace the repo link if it stays private, or open just `docs/`
- [ ] Record the retraction-beat GIF for the hero (`scripts/demo_context_for.sh` has the scene)
- [ ] Re-check the F1 number against `MVP-STATUS` at publish time
