# TraceMind — YC Application Draft

**Date:** 2026-07-28
**Companion to:** `YC-READINESS-2026-07-28.md` (read that first — it explains
why several answers below are deliberately uncomfortable)

Answers are written to the standard YC application form. Where the honest
answer is weak, it is written weak — **do not** patch these with optimism. YC
partners read thousands of these and the single fastest way to lose is to be
caught inflating. Notes in `>` blockquotes are for you, not for the form.

---

## Company

**Name:** TraceMind

**Describe what your company does in 50 characters or less.**

> Pick one. First is my recommendation — it's concrete and it's the thing
> nobody else says.

- `Local AI memory that tells you when it changed` (46)
- `On-device memory layer for your AI assistants` (45)
- `Your AI's memory, on your laptop, never cloud` (45)

**Company URL:** *(none yet — see §Gaps)*

---

## What is your company going to make?

TraceMind is a memory layer for the person using AI assistants, not for the
company building them. It runs entirely on your laptop — no network in the
request path — and plugs into Claude Code, Cursor, Goose, and any other MCP
host through a single verb: given a topic, hand back a token-budgeted, cited
brief of everything you already know about it.

It ingests what you actually do — text, URLs, clipboard, shell history, files —
into a knowledge graph with an immutable audit trail. Every fact carries
provenance you can inspect.

The part that is different: it maintains beliefs, not just documents. When two
things you told it conflict — "the final is in New Jersey" and later "the final
is in Los Angeles" — it doesn't silently overwrite or return both. It detects
the clash through a truth-maintenance layer, retracts the superseded belief,
and surfaces that as an event you can see and act on. Memory that revises
itself in the open, rather than quietly drifting.

---

## How far along are you?

Working software, no users.

Four and a half months of full-time solo work: 101,501 lines of Rust across 30
crates, 1,240 tests, shipping as a CLI, an MCP server, an ambient capture
daemon, and a desktop app. Retrieval scores 50.32 F1 on held-out conversations
using real on-device embeddings. There is a self-improvement loop (GEPA) that
tunes the retrieval policy against executed rollouts rather than a proxy.

> **Do not soften this next paragraph.** It is the strongest thing in the
> application, because it is the thing they cannot verify about anyone else.

The honest version: I built far more than I validated. I recently ran a review
of my own project that concluded a large amount of capability had been built
against a mechanism no real user had ever exercised. Acting on it, I cut the
MCP surface from 51 tools to a core 6, and I retracted my own headline
retrieval number — 75.49 F1 — after discovering it had been tuned on the same
20 questions it was scored against. The real number on fresh conversations is
50.32. I also built a regression gate specifically to catch the class of error
that had hidden a dead retrieval path for four releases.

I would rather tell you that than have you find it.

---

## How long have you been working on this?

Since 2026-03-09 — about 4.5 months, full-time, solo. 140 commits.

---

## Tech stack

Rust workspace (30 crates). SQLite for both the knowledge graph and the vector
store. BGE-small-en-v1.5 ONNX embeddings (384-dim) on-device via fastembed,
ColBERT reranking (mxbai-edge-colbert). JTMS truth-maintenance layer for belief
revision. Tiered answer layer: extractive → local Qwen 2.5 1.5B via llama.cpp →
Apple FoundationModels on macOS 26+. MCP over JSON-RPC for host integration.
Tauri 2 desktop app. No cloud services in the request path.

---

## Are people using your product?

**No.** Zero design partners with real sessions today.

> This is the answer that will most likely sink a Fall 2026 late application,
> and there is no way to write around it. See the readiness memo §6 — the plan
> is to make this answer "5 partners, N sessions, X% still active at day 14"
> before the Winter 2027 deadline. If you are reading this after that work is
> done, **replace this section with the real numbers and delete this note.**

---

## Do you have revenue?

No.

---

## Why did you pick this idea? How do you know people need it?

I use AI assistants for most of my working day, across several windows and
several tools, and every one of them starts from zero. I re-explain the same
context constantly. The tools that claim to fix this either want my data in
their cloud or forget anything structural about how my beliefs changed.

> **Weakest answer in the application, and they will notice.** "I had this
> problem myself" is the most common founder story YC sees. It is only
> persuasive when paired with evidence that others have it too. Right now
> that evidence is: nothing.
>
> Fix before submitting seriously: talk to 20 Claude Code power users, and
> replace this section with what they told you — specific quotes, specific
> workflows, how they work around it today. That converts the weakest answer
> into a strong one, and it is ~2 weeks of work.

---

## Who are your competitors? What do you understand that they don't?

Mem0 (YC-backed, $24M, ~41k GitHub stars, exclusive memory provider in the AWS
Agent SDK), Letta (~$10M seed, OS-inspired tiered agent memory), Zep with its
open-source Graphiti temporal knowledge graph, and Supermemory on the personal-
vault side.

Two of these are genuinely close and I won't pretend otherwise. Zep's Graphiti
already models how facts change over time — the same category of problem as my
truth-maintenance layer. Supermemory already targets individual knowledge
workers through MCP.

What I think is under-appreciated: **all of them treat belief revision as an
internal retrieval detail, and none of them show it to the user.** When your
memory changes its mind, you find out implicitly, by getting a different
answer. My bet is that the moment a memory system becomes legible about its own
revisions — "I used to think X, you told me Y, I dropped X, here's why" — is
the moment people start trusting it with the messy, ambient, high-value context
they currently withhold. Trust is the actual bottleneck on memory products, not
recall quality.

The second thing: running fully on-device isn't a privacy feature, it's a
permission structure. Because nothing leaves the laptop, I can ingest a
person's clipboard, shell history, and browsing — inputs a cloud vendor cannot
ask for without a compliance conversation. The constraint buys access to the
richest signal.

> This is a **hypothesis**, and phrase it as one. Claiming it as a known truth
> with 0 users invites the obvious follow-up. Stating it as a falsifiable bet
> you are actively testing is much stronger.

---

## How will you make money?

Individual subscription for the local product ($10–20/mo), with a team tier
where the graph is shared across a workspace but data still stays on member
devices. The market is every knowledge worker who uses an AI assistant daily —
already tens of millions and growing.

> Thin, and knowingly so. Don't over-engineer it; at pre-seed YC cares far more
> that you can articulate *who pays and why* than about a five-year model.
> The version to avoid is "we'll figure out monetization later."

---

## One metric that best reflects progress

**Day-14 retention of design partners** — of the people who installed it, how
many still have it running two weeks later, with real sessions.

Today that number is undefined because the denominator is zero. It is the
number I am organizing the next ten weeks around.

---

## Category

Developer tools / AI infrastructure. (Consumer-adjacent, but the first users
are developers using AI coding assistants.)

---

## Equity / team

Solo founder.

> Address it, don't hide it. Suggested phrasing:

I'm solo. I know that's a negative signal and I'd rather engage with it than
leave it unsaid. I've shipped a large, coherent, tested system alone in 4.5
months, which I think answers the "can he execute" version of the concern. The
version I take seriously is the co-founder-as-check-on-judgment one — and this
project's own history is evidence for it: I spent months building capability
before validating demand, and a co-founder would likely have caught that
sooner. I'm actively looking, and I'd want YC's help with it.

---

## Videos

**Founder video (1 min).** Talk to camera. Structure: who you are → the
specific moment you got annoyed enough to build this → what you shipped in 4.5
months → what you got wrong and fixed. End on the wedge sentence. No slides.

**Demo video (1 min).** See the script below. **Do not use the 19-surface
product tour** — it demonstrates breadth, and breadth is your weakness, not
your strength.

### Demo script — the retraction beat

| Time | On screen | Voiceover |
|---|---|---|
| 0:00–0:08 | Terminal. `tracemind ingest "FIFA World Cup 2026 final located in New Jersey"` | "TraceMind runs entirely on my laptop. I tell it something." |
| 0:08–0:16 | `tracemind ingest "FIFA World Cup 2026 final located in Los Angeles"` — the `⚠ Retraction:` line fires | "Later, I tell it something that contradicts that. Watch." |
| 0:16–0:28 | Zoom the retraction line | "It didn't overwrite. It didn't hand back both. It noticed the clash, retracted the belief it no longer holds, and told me — so I can reconcile it." |
| 0:28–0:40 | Claude Code. Bind `memory_context_for`, ask about the World Cup; cited brief appears | "Any MCP host gets this through one verb: a cited brief of what I already know." |
| 0:40–0:52 | Scroll citations → `tracemind trace` audit log | "Every claim is cited. Every ingest and query is in an immutable audit log." |
| 0:52–1:00 | Network monitor at zero | "None of that left the machine. That's the point — it's why it's allowed to see my clipboard and my shell history." |

---

## Gaps to close before submitting

| Gap | Why it matters | Effort |
|---|---|---|
| **0 users** | Sinks the application on its own | 10 wks (readiness memo §6) |
| No landing page / URL | Form asks; absence reads as unserious | 1 day |
| "How do you know people need this" is unevidenced | Weakest answer | 2 wks of user conversations |
| Wedge sentence not chosen | Five framings across four months | 1 day — pick one |
| `docs/INVESTOR_DECK.pdf` may still quote 75.49 | An inflated number in a deck after retracting it is worse than never retracting | 1 day |
| `CLAUDE.md` says 17 crates; there are 30 | Investors read repos | 5 min |
| Founder + demo video not recorded | Required | 1 day |

---

## Submission strategy

Per the readiness memo §8: submit a **timeboxed one-day** late Fall 2026
application using these answers as-is, honestly, including the zero. Treat it
as a free lottery ticket, not the real attempt. Then spend ten weeks turning
"Are people using your product? No" into a real number, and submit a genuinely
strong Winter 2027 application in late October / early November.
