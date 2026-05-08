# TraceMind — Product Portfolio

**Status**: proposal — 2026-05-07
**Author**: Aaditya + Claude (co-founder brainstorm)
**Predecessors**: `BRAIN_ARCHITECTURE.md`, `INTENT_SYSTEM.md`, `PHASE4_DELIGHT.md`
**Companion**: `UNIFIED_ARCHITECTURE.md` (shared engine), `PHASE5_CONVERGENCE.md` (implementation roadmap)

---

## 0. What is intent?

Intent is the invariant that holds still while everything else moves.

Data changes **form** — code gets refactored, a verbal commitment becomes a
written spec, a Slack message becomes a Jira ticket. Data changes
**modality** — a screenshot captures what words can't, a voice memo
replaces a typed note, a git diff replaces a conversation. Data changes
**validity over time** — what was true yesterday isn't today; what the
agent believed at 2pm was retracted at 5pm.

**Intent persists through all three transformations.** When you say "ship
v2 by Friday," that intent survives whether it's captured as text, voice,
a commit message, or a calendar event. When you refactor a function, the
intent ("validate payment amounts before charging") survives even though
every line of code changed. When a fact is retracted, the intent behind
the original belief ("we chose Postgres because we needed ACID") remains
historically valid even after the decision is reversed.

### 0.1 Intent is not a moment — it's an arc

Nobody says "I hereby commit to shipping v2 by Friday" in isolation.
There's a reason they said it, a feeling behind it, the commitment
itself, what they actually did, and what happened. Intent is the full
arc:

```
  Need → Sentiment → Commitment → Action → Outcome
  ~~~~   ~~~~~~~~~   ~~~~~~~~~~   ~~~~~~   ~~~~~~~
  WHY    HOW I FEEL  WHAT I       WHAT I   WHAT
  I care ABOUT IT    PLAN TO DO   DID      HAPPENED
```

Each phase is a first-class primitive:

- **Need** — the underlying driver. "I need faster deployments." Needs
  are pre-commitment: they're the *why* behind every decision. They
  recur, compound, and reveal what actually matters to someone over time.
  A need doesn't require a plan — it exists as soon as a gap between
  reality and desire is felt.

- **Sentiment** — the emotional context. "I'm frustrated with CI" or
  "I'm excited about this new vendor." Sentiment colors every decision
  but is rarely captured. It changes over time — the frustration that
  drove a migration decision might turn to satisfaction once the
  migration lands. Tracking sentiment lets the system understand not
  just *what* happened but *how the user felt about it*.

- **Commitment** — the forward-leaning declaration (already shipped as
  TraceMind's core primitive). "I'll migrate to GitHub Actions by June."
  This is intent crystallized: a statement with options considered, a
  chosen path, stakes, a horizon, and expected outcome.

- **Action** — what was actually done. "Opened PR #47 with the
  migration." Actions are detected from capture streams (git commits,
  completed tasks, sent messages, deployed code). The gap between
  commitment and action is where insight lives — "you said you'd do X
  but you actually did Y" is one of the most valuable observations a
  system can make.

- **Outcome** — what happened (already shipped). "Deploy time dropped
  60%." The resolution that closes the loop and feeds the world model.

The power is in the *connections*: when the user asks "why did we switch
CI?", the system traces through outcome → action → commitment →
sentiment → need and grounds the full story.

### 0.2 Properties of intent

1. **The reason a thing exists** — not the thing itself. The function
   exists to validate payments. The commitment exists because the founder
   needs to ship faster. The belief exists because the data suggested
   Postgres scales.

2. **Recoverable from evidence** — intent isn't mystical. It's grounded
   in the context at the time: what was known, what was felt, what
   options existed, what was chosen, what was expected. TraceMind's
   `ContextSnapshot` is a freeze-frame of the full arc at decision time.

3. **The unit of reasoning** — you don't reason about lines of code; you
   reason about what the code is trying to do. You don't reason about
   individual facts; you reason about what they imply for a decision.
   Intent is the natural unit of human reasoning, and therefore the
   natural unit of a system that helps humans think.

4. **Multi-layered** — a single intent has depth. The need ("ship
   faster") is more stable than the sentiment ("frustrated today") which
   is more stable than the commitment ("migrate by June") which is more
   stable than the action ("PR #47"). The system reasons at the right
   layer for each question: needs for "what matters to you?", sentiments
   for "how do you feel about this?", commitments for "what did you
   decide?", actions for "what did you do?", outcomes for "what happened?"

5. **Compounding** — intent is not static. Every resolved intent arc
   teaches the system something about how this user's intents play out.
   The outcome of one arc becomes context for the next need. More
   arcs → more outcomes → better predictions → more trust → more
   intents captured. This is the flywheel.

### 0.3 How this extends the existing primitive

TraceMind's shipped `Commitment` primitive already captures the middle
of the arc (commitment + outcome). Phase 5 extends it in both
directions:

- **Upstream**: Need and Sentiment extraction from existing capture
  streams. Needs are mined from phrases like "I need", "we have to",
  "the problem is." Sentiments are inferred from word choice and
  valence (Tier-1 LLM-assisted or heuristic fallback).

- **Downstream**: Action detection from capture streams. Git commits,
  deployed code, sent messages, completed tasks — matched to open
  commitments by embedding similarity and temporal proximity.

- **Connections**: edges in the graph link Need → Sentiment →
  Commitment → Action → Outcome. The chain is traversable, auditable,
  and grounded.

The three products in this portfolio extend intent tracking across
modalities (TraceMind), across time with formal belief revision
(Engram), and across code transformations with semantic preservation
(Rosetta).

---

## 1. The thesis

Three products. One engine. One primitive: **intent**.

Every product answers the same question from a different angle:

| Product | Question it answers |
|---|---|
| **TraceMind** | "What did I intend, what happened, and what should I expect next?" |
| **Engram** | "What did the agent believe, when did it change, and why?" |
| **Rosetta** | "What does this code intend, and does the transformation preserve it?" |

They share infrastructure in a way that compounds:

| Shared capability | TraceMind | Engram | Rosetta |
|---|---|---|---|
| Intent arc (need→sentiment→commitment→action→outcome) | Full personal decision lifecycle | Agent goals→beliefs→actions→observations | Code requirements→design intent→implementation→tests |
| Bitemporal graph | "What did I believe when I made that decision?" | "What did the agent know at time T?" | "What was the code's intent before the refactor?" |
| Truth maintenance | Contradiction detection across commitments | Automatic belief revision when facts change | Detecting when a code change violates documented intent |
| Cross-modal reasoning | Screenshot + commit + Slack → grounded chain | Agent reasons across tool outputs (JSON + images + logs) | Code + tests + docs + PR discussions → semantic understanding |
| Sentiment tracking | "How did I feel about this vendor?" → context for decisions | "Agent confidence in this belief?" → evidence strength | "Tech debt frustration" → migration priority signal |
| World model | Predict your next decision | Predict what the agent will need next | Predict what a migration will break |

An improvement to the truth maintenance system makes all three products
better simultaneously. An improvement to cross-modal encoding makes all
three products see more. That's the compounding moat.

---

## 2. Product 1 — TraceMind (consumer, Phase 1)

### One line

A system of intents — captures what you need, how you feel about it,
what you commit to, what you actually do, and what happens — then
anticipates what's next from your own track record. Across every
modality, grounded in time. All on your machine.

### What exists today

- 20-crate Rust workspace, 10 shipped intent-system features (TM-INTENT-001 through 010)
- Commitment → Outcome → Anticipation pipeline with 3 predictive layers (L1/L2/L3)
- World model (f_topic + f_outcome), pattern detector, calibration auto-quiet
- Bandit-driven retrieval (5 arms, LinUCB), ColBERT rerank, ONNX embeddings
- MCP server (memory_store, memory_query, memory_commit, memory_resolve, memory_recommend, etc.)
- Capture daemon (clipboard + shell + MCP), daily brief, insights panel
- Tauri desktop app shell, CLI (tracemind)

### What Phase 5 adds (see `PHASE5_CONVERGENCE.md`)

1. **Cross-modal reasoning** — the system reasons across screenshots,
   audio, code, documents, and text. Not "embed everything into one
   vector space" — actual multi-hop reasoning chains that cross
   modalities. "Find the screenshot where the bug appeared, link it to
   the commit that caused it, surface the Slack thread where we discussed
   it."

2. **Bitemporal belief tracking** — every fact in the graph carries
   valid-time (when it was true in reality) and transaction-time (when
   the system learned it). The system can answer "what did I believe last
   Tuesday?" and "when did I learn that the vendor was unreliable?"

3. **Truth maintenance** — automatic propagation of belief revisions.
   Retract a supporting fact → dependent beliefs are flagged. Surface
   contradictions before they become costly.

### Audience

Solo founders and indie operators (wedge). Researchers, PMs, and
knowledge workers (expansion). Three persona packs at 1.0:
Founder / Researcher / Journaler.

### Business model

Free for personal use. Prosumer tier ($X/mo) for advanced features:
voice mode, L3 recommendations, multi-device sync, priority model
downloads.

### Surfaces

CLI, MCP server, Tauri desktop app, mobile (iOS/Android via UniFFI).

---

## 3. Product 2 — Engram (infrastructure, Phase 2)

### One line

Agent memory that knows what was true *when*. Bitemporal beliefs, causal
consistency, contradiction detection. The memory substrate AI agents
deserve.

### The problem

AI agents forget everything between sessions. Current "agent memory"
solutions (Mem0, Letta, Zep) are vector stores with timestamps — they
can retrieve facts but cannot:

- Tell you what the agent believed at a specific past point in time
- Detect when a new fact contradicts an existing belief
- Automatically propagate retractions (if fact A is retracted, and belief
  B depended on A, B is flagged)
- Reason about the temporal validity of facts (fact X was true from March
  to April, then superseded by fact Y)
- Maintain causal consistency across a chain of agent actions

These are not nice-to-haves. They're the difference between an agent
that accumulates knowledge and an agent that accumulates confusion.

### What it is

A Rust SDK + MCP server exposing TraceMind's engine as infrastructure
for agent developers. The API is belief-native, not document-native:

```rust
let mem = Engram::open("./agent-memory")?;

// Assert a belief with evidence
let belief = mem.assert(
    "stripe webhook requires amount_cents",
    Evidence::from_code("webhook.rs:47"),
)?;

// Later, the world changes
mem.retract(
    belief.id,
    "v2 API made amount_cents optional",
    Evidence::from_url("stripe.com/docs/changelog"),
)?;

// Query what the agent knew at a past point
let world = mem.world_at(valid: march_15, known_as_of: april_1)?;
assert!(world.believes("stripe webhook requires amount_cents"));

// Find contradictions
let issues = mem.contradictions()?;
// → "You believe X (from source A) and ¬X (from source B)"

// Temporal query
let timeline = mem.history_of("stripe webhook")?;
// → [believed from 2026-01-15 to 2026-04-01, retracted, reason: ...]
```

### What it shares with TraceMind

| TraceMind component | Engram equivalent |
|---|---|
| `tm-graph` (SQLite KG) | Same store, bitemporal extensions |
| `tm-intent::Need` | `Goal` (what the agent is trying to achieve) |
| `tm-intent::Sentiment` | `Confidence` (agent's certainty in a belief — maps to evidence strength) |
| `tm-intent::Commitment` | `Belief` (generalized — no horizon/stakes, adds justification DAG) |
| `tm-intent::Action` | `AgentAction` (tool calls, API requests, code changes the agent made) |
| `tm-intent::Outcome` | `Observation` (evidence that confirms/contradicts a belief) |
| `tm-reflect::PatternDetector` | `ConsistencyChecker` (runs TMS over belief graph) |
| `tm-reason::ChainBuilder` | `ReasoningChain` (multi-hop over beliefs with temporal awareness) |
| `tm-types::Trace` | `AuditEntry` (immutable provenance log) |

### Technical moat

- **Bitemporal data model** — every fact has valid-time and
  transaction-time. Most databases support one; supporting both with
  efficient indexing and query is hard systems work.
- **Justification-based Truth Maintenance System (JTMS)** — from
  classical AI, barely used in modern ML. Tracks which beliefs support
  which other beliefs. When a foundation belief is retracted, all
  dependent beliefs are automatically flagged. This is O(beliefs) not
  O(beliefs^2) with the right data structure.
- **Incremental consistency checking** — contradictions detected at
  assertion time, not batch. The agent learns immediately when a new
  fact conflicts with existing knowledge.
- **Cross-modal evidence** — beliefs can be grounded in text, code,
  images, structured data. The evidence chain is heterogeneous.

### Audience

Agent developers building with Claude, GPT, open-source models.
Framework integrators (LangChain, CrewAI, AutoGen, Semantic Kernel).
Enterprise teams building internal agents that need auditable memory.

### Business model

Open-source core (Rust SDK, local use). Metered cloud API for managed
Engram instances. Enterprise license for on-prem + SLA + support.

### Distribution

Ship as an MCP server — every Claude Code user, every Cursor user can
plug in immediately. Publish crate on crates.io. npm/PyPI wrappers via
FFI. Day-one compatibility with the fastest-growing agent ecosystems.

---

## 4. Product 3 — Rosetta (dev tool, Phase 3)

### One line

Understands code at the intent level. Migrates, refactors, and reasons
about code by what it *means*, not what it *says*. Every transformation
preserves intent with proof.

### The problem

Code migration today is either:
1. **Manual** — expensive, slow, error-prone, doesn't scale
2. **LLM-generated** — fast but unreliable; transliterates syntax rather
   than preserving intent; no correctness guarantees; hallucinations in
   edge cases

Neither approach understands *why* the code exists. A Python class with
a decorator-based validation pattern gets translated to Go as a struct
with the same field names — but the validation intent is lost because Go
doesn't have decorators. The migration "worked" syntactically but failed
semantically.

### What it is

A dev tool (CLI + IDE extension) that:

1. **Extracts intent** from code using cross-modal reasoning — the code
   itself, its tests, its docs, its PR discussions, its commit messages,
   its comments. Builds an intent graph: what each module/function/type
   is *for*, not just what it *does*.

2. **Preserves intent through transformations** — refactoring, migration
   (Python→Rust, Java→Go, etc.), simplification, decomposition. The
   output is idiomatic in the target language while preserving the
   semantic intent of the source.

3. **Proves preservation** — generates property-based tests that verify
   the transformation preserved intent. "The old code validated payments
   before charging; the new code still validates payments before
   charging." Not just type-checking — behavioral equivalence testing.

4. **Tracks intent drift over time** — uses bitemporal tracking to
   notice when intent drifts. "This function was originally for payment
   validation (commit abc123, 6 months ago). It's now also doing rate
   limiting (commit def456, 2 weeks ago). The intent has expanded
   silently."

### What it shares with TraceMind

| TraceMind component | Rosetta equivalent |
|---|---|
| `tm-intent::Need` | `Requirement` — the underlying need this code satisfies |
| `tm-intent::Sentiment` | `TechDebtSignal` — frustration/satisfaction signals around code areas |
| `tm-intent::Commitment` | `CodeIntent` — what this code unit is FOR |
| `tm-intent::Action` | `Implementation` — the actual code written to fulfill the intent |
| `tm-intent::Outcome` | `TransformationResult` — did the migration preserve intent? |
| `tm-modal` (cross-modal encoding) | Code + tests + docs + PRs → unified intent graph |
| `tm-tms` (truth maintenance) | Intent consistency checking across transformations |
| `tm-temporal` (bitemporal) | Intent drift detection over git history |
| `tm-reason::ChainBuilder` | Cross-file intent reasoning chains |
| `tm-graph` (knowledge graph) | Codebase-as-graph: modules, functions, types, intents, dependencies |

### Technical moat

- **Intent extraction from cross-modal evidence** — understanding code
  intent requires reading the code, the tests, the docs, the PR
  discussion, and the commit history together. Nobody else does
  cross-modal reasoning across all of these simultaneously.
- **Semantic diff** — "this commit changed authorization from role-based
  to attribute-based" instead of "changed 47 lines in 12 files." Requires
  deep program analysis + learned intent models.
- **Behavioral equivalence verification** — automatically generated
  property tests that prove the transformation preserved intent. Not
  just "it compiles" — "it does the same thing."
- **Intent-aware AST transformations** — tree-sitter parsing +
  intent-annotated AST → idiomatic output in the target language.
  The transformation operates on intent-annotated trees, not raw syntax.

### Audience

Engineering teams doing language migrations (Python→Rust, Java→Go,
monolith→microservices). Platform teams maintaining polyglot codebases.
Individual developers doing large refactors.

### Business model

Team license ($X/seat/mo). Enterprise license for large-scale migrations
with SLA. Open-source CLI for individual use on small codebases.

### Distribution

VS Code / JetBrains extension. CLI. GitHub Action for CI integration
("flag PRs where intent drifted"). MCP tool for Claude Code / Cursor
integration.

---

## 5. Portfolio economics

### Shared engine investment

Every dollar of engineering on the shared engine (Layer 0) pays
dividends across all three products:

| Engine improvement | TraceMind benefit | Engram benefit | Rosetta benefit |
|---|---|---|---|
| Faster bitemporal queries | Snappier time-machine queries | Lower API latency | Faster intent-drift scans |
| Better cross-modal encoding | Richer reasoning chains | More evidence types for beliefs | Better intent extraction from PRs/docs |
| Improved TMS | Fewer false contradiction alerts | More reliable belief revision | More accurate intent-consistency checking |
| World model improvements | Better L2/L3 predictions | Better "what will the agent need next" | Better "what will this migration break" |

### Revenue timeline

| Phase | Product | Revenue model | Expected timing |
|---|---|---|---|
| 1 | TraceMind | Free → prosumer upsell | Month 1+ (organic growth) |
| 2 | Engram | Metered API + enterprise | Month 4+ (after engine proves itself) |
| 3 | Rosetta | Team license + enterprise | Month 8+ (needs most R&D) |

### Risk allocation

TraceMind is the *proving ground* — it stress-tests the engine with real
users before Engram or Rosetta depend on it. If the cross-modal
reasoning or bitemporal tracking has bugs, we find them in a consumer
product where the cost of failure is a bad daily brief, not a broken
production agent or a botched code migration.

---

## 6. Sequencing (Approach C — Staged Unfold)

```
Month 1-3      Month 4-6        Month 7-9        Month 10+
────────────   ──────────────   ──────────────   ──────────────
Phase 1        Phase 2          Phase 3          Compounding

TraceMind      + Engram SDK     + Rosetta CLI    All three
gets cross-      extracted        ships with       products
modal +          from engine.     intent           sharing
bitemporal.      MCP server       extraction       improvements.
Engine           + crates.io.     + semantic       Engine is
stress-tested    Revenue          diff +           the moat.
by real          from agent       proof gen.
users.           devs.
```

Each phase validates the engine before the next product relies on it.
TraceMind is the flagship that proves the pattern. Engram is the
infrastructure that monetizes the engine. Rosetta is the dev tool that
showcases the most ambitious capability (intent-preserving code
transformation).

---

## 7. What unifies the portfolio

It's not "three random AI products." It's one conviction:

> **Intent is the right unit of reasoning for human-AI systems.**

- For individuals (TraceMind): intent = the full arc from need to outcome — why you care, how you feel, what you decided, what you did, what happened
- For agents (Engram): intent = goals → beliefs → actions → observations — the full agent decision lifecycle with causal consistency
- For code (Rosetta): intent = requirements → design decisions → implementations → tests — why code exists, not just what it does

All three track intent through time, across modalities, with
provenance. All three use the same engine. All three get better as
the engine improves.

The portfolio is a bet that **intent-native systems** are a category —
and that the team that builds the best intent engine wins across
multiple surfaces. That's the real moat: not any one product, but the
engine underneath all of them.

---

## 8. Decisions log

Confirmed 2026-05-07 in brainstorm session:

1. **Three-product portfolio**: TraceMind (consumer), Engram (infra), Rosetta (dev tool). ✅
2. **Staged unfold (Approach C)**: Phase 1 merges into TraceMind, Phase 2 extracts Engram, Phase 3 ships Rosetta. ✅
3. **Shared engine (Layer 0)** is the moat. ✅
4. **Intent is the unifying primitive** across all three products. ✅
5. **Intent is the full arc**: Need → Sentiment → Commitment → Action → Outcome. Not just the commitment — the complete decision lifecycle. (Option C, confirmed 2026-05-07). ✅
6. **Phase 4 (PHASE4_DELIGHT.md) and Intent System (INTENT_SYSTEM.md) remain canonical** for their scope. This doc adds the portfolio layer above them. ✅
7. **Implementation details** live in `PHASE5_CONVERGENCE.md`. ✅
