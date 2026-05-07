# TraceMind — investor brief

> **For:** a friend who's looking. Be honest, not polished.
> **Date:** 2026-05-06
> **Author:** Aaditya
> **Status:** Pre-seed, no raise yet, asking for thinking time + intros, not a check.

---

## Slide 1 — One line

**TraceMind is a local-only memory OS for humans and AI agents.**
Captures intent, predicts outcomes, and earns trust on-device.
Cloud memory is the default; we're betting privacy is the moat.

---

## Slide 2 — The problem (in one paragraph)

Every AI assistant today forgets. The fixes on the market — Mem0,
Honcho, Letta, Zep, SuperLocalMemory — solve forgetting by sending
your data to their cloud. That's a fine MVP. It's a terrible long-term
position once "memory" becomes the most sensitive surface in your
digital life: meeting notes, intents, decisions, side comments,
clipboard. The data moat goes the wrong direction.

**The opening:** the user is already on a powerful machine. The
right answer is to build the brain there, and only there.

---

## Slide 3 — Our wedge

We are not a "memory database." We are a **system of intents.**

The primitive is a `Commitment` — a recorded statement of intent or
decision with metadata you can predict over later:

```
kind        intent | decision | hypothesis
stakes      low | medium | high | reversible
horizon     when this should resolve by
expected    what success looks like
```

You commit to something. Time passes. You resolve it. The polarity
becomes training data for a personal world model.

**Why this is the wedge:** notes apps are a saturated category.
Decision-tracking with a model that learns *your* track record is not.
And it composes naturally with any LLM — we expose 21 tools over MCP.

---

## Slide 4 — How it works (the brain in 4 layers)

```
┌─────────────────────────────────────────────────────┐
│  Sensory   →  clipboard / shell / MCP intake        │
│  buffer       (tm-capture daemon, always-on)        │
├─────────────────────────────────────────────────────┤
│  Memory    →  6 layers (working / episodic /        │
│               semantic / procedural / sensory /     │
│               intent) — all on-device SQLite        │
├─────────────────────────────────────────────────────┤
│  Cognition →  10 ops (attention / recall /          │
│               association / reasoning /             │
│               consolidation / dreaming / pattern    │
│               detection / synthesis / self-monitor /│
│               world modelling)                      │
├─────────────────────────────────────────────────────┤
│  Surfaces  →  CLI · MCP server (21 tools) ·         │
│               capture daemon · Tauri desktop app    │
└─────────────────────────────────────────────────────┘
```

- **17 Rust crates**, ~12k LOC, 163 tests passing.
- **All data in `~/.tracemind/`** — one SQLite file plus a JSONL
  audit trail. The user can `rm -rf` it. That's the contract.
- **No network in the request path.** Network is opt-in, only for
  initial model downloads.

---

## Slide 5 — What's shipped

**Memory pipeline (Act 1 of the demo):**
- BGE-small ONNX embeddings (384-dim, real, on-device)
- GLiNER NER for entity extraction
- SQLite graph + vector store
- Immutable trace log (every ingest gets a UUID)
- ColBERT reranking auto-downloads on first use

**Intent system (Act 2):**
- `Commitment` + `Outcome` + `Anticipation` types
- CLI: `tracemind commit` / `commitments` / `resolve` / `brief`
- MCP tools: `memory_commit`, `memory_resolve`, `memory_brief`
- Daily brief surface — calm-mode, no nagging

**World model (Act 3-4):**
- Logistic regression on 22-dim metadata feature space
- Auto-retrains on every resolve
- Calibration view that goes silent when miscalibrated
- (This is the "earned trust" loop)

**Integration (Act 5):**
- MCP server with 21 JSON-RPC tools
- Stdio-only — no network, no daemons
- Any MCP-aware client (Claude Code, Cursor, custom agents) plugs in

**The proof:** there's a 6-minute screen recording of all five acts
running on real data with real embeddings. I'll send it.

---

## Slide 6 — What's good (honest version)

1. **The architecture is right.** 6 memory types and 10 cognitive ops
   match the brain literature, not a database schema. Hard to copy
   without rebuilding from scratch.

2. **Local-only is real, not marketed.** Every storage layer is SQLite
   on the user's machine. No "encrypted cloud sync that's actually a
   default-on cloud sync."

3. **Real eval harness.** We run LoCoMo (the published memory
   benchmark) with a CI gate that fails any PR dropping >0.5 F1.
   That's discipline most pre-seed AI teams don't have.

4. **The wedge is sharp.** "System of intents" is a category that
   doesn't exist yet. Notes apps are saturated; commitment-tracking
   with a personal predictive model is empty.

5. **The brain extends naturally.** Same crates power the CLI, the
   MCP server, the capture daemon, and the desktop app. No
   architectural rewrite when we add a new surface.

---

## Slide 7 — What's bad (also honest)

1. **LoCoMo F1 is 25.7. The target is 85.**
   That's a real gap. Mem0 / Letta / Zep clear 70+ because they use
   cloud LLMs as their synthesis layer. We've been on Tier-0
   (extractive, template-based) the whole time. The fix is shipping
   Tier-1 (Qwen 2.5 1.5B Q4, ~900MB on-device LLM, already scaffolded).
   This is the single biggest lever.

2. **The world model is logistic regression.**
   It works (calibration loop runs, brief shows the outlook line),
   but logistic regression on 22 features is not "an AI moat." We
   have an MLP + held-out validation in WIP, but it's not landed yet.

3. **GLiNER NER had a non-determinism paper cut.** *(fixed 2026-05-06)*
   Same sentence, different runs, used to give 0 to 5 entities.
   Root cause: ORT thread scheduling + a 0.30 sigmoid cutoff sitting
   right where logits flutter. Fix landed: deterministic ORT session
   (single-thread, deterministic kernels), 0.05 threshold deadband,
   stable NMS tiebreakers. Costs ~1 F1; buys reproducible output.

4. **No multi-user, no sync, no collaboration.**
   By design today, but every investor will ask "team memory?"
   We need a clean answer: Phase 6, ACL + REST API.

5. **Tauri desktop app builds but doesn't ship.**
   Pre-existing build script issue. We exclude it from CI test runs.
   Not investor-blocking, but not polished.

6. **No funded user base.** GitHub stars are not customers. The
   honest read is we're optimizing the engine before we have proof
   that anyone wants this engine. (Counter-read: privacy-first is
   a "you don't realize you want it until cloud memory burns you"
   category — we're early on purpose.)

---

## Slide 8 — What needs to improve (the cognition roadmap)

Ten moves, ranked by impact / effort. Full version in
`docs/PHASE4_DELIGHT.md` §9.

### Tier 1 — biggest unlocks (do first)

1. **Ship Tier-1 synthesis (Qwen 2.5 1.5B Q4).** Single biggest lever.
   Every cognition op looks 3× smarter for free. Effort: small.
2. **Real reward signal into the bandit.** Today the bandit is
   starving on weak proxies. One keypress feedback in the brief
   compounds for weeks. Effort: small.
3. **Query rewriting before recall.** Tier-1 expands "how does X
   earn trust?" into 3 queries, fuses with RRA. ~5–8 F1 on
   multi-hop. Effort: small.

### Tier 2 — structural fixes

4. Surprisal-gated ingestion (skip low-info captures).
5. Reasoning chains with beam search, not greedy.
6. Calibration via Platt scaling on world-model probabilities.

### Tier 3 — moat work

7. Replace WL-kernel association with a learned graph embedding (GNN).
8. Consolidation via Louvain/Leiden community detection nightly.
9. Pattern detector → factorization machine over interaction
   features.
10. Working-memory ring (last N turns + retrievals fed back).

**The one to bet on:** #1, Tier-1 synthesis. Everything else is a
multiplier on a base. Without Tier-1 the base is templates.

---

## Slide 9 — The plan (sprints A–G, canonical in `INTENT_SYSTEM.md` §10)

| Sprint | Focus                                       | Status      |
|--------|---------------------------------------------|-------------|
| A      | Intent primitives + MCP tools               | DONE        |
| B      | Working memory ring + capture miner         | IN FLIGHT   |
| C      | World model (logistic) + calibration loop   | DONE        |
| D      | Pattern detector + L2/L3 anticipations      | NEXT        |
| E      | Tier-1 synthesis default-on (THE big sprint)| HIGHEST ROI |
| F      | Dreaming / consolidation / surprisal        | DESIGNED    |
| G      | Companion UX (voice, narrative, garden)     | SCOPED      |

The cognition roadmap items map onto these — see PHASE4_DELIGHT.md
§9.6 for the mapping table.

---

## Slide 10 — The moat (in three lines)

1. **Privacy by construction.** Not a feature, not a setting. The
   data physically lives on the user's machine.

2. **Brain-shaped architecture.** 6 memory types × 10 cognitive ops
   is hard to clone without a ground-up rewrite. Cloud memory
   competitors built database schemas; we built an architecture.

3. **The trust loop.** The world model goes silent when it's
   miscalibrated. That's the opposite of every notification-driven
   product. Earning the right to speak, by performance, is the
   long-term differentiator.

---

## Slide 11 — What I'm asking from you

Not a check. Not yet. Three things:

1. **Read this and tell me what's wrong.** You've seen more decks
   than I have. The honest read.
2. **Watch the 6-minute demo.** Tell me whether Act 3 (the world
   model preflight moment) lands as the magic moment I think it is,
   or if I'm fooling myself.
3. **Two intros if any of this resonates:**
   - one ML-flavoured solo founder I should compare notes with
   - one privacy/security-aware angel who'd get the wedge
     without me having to explain why local matters

---

## Slide 12 — Closing

The boring memory-OS race is already lost to Mem0 / Honcho.
The *companion* race hasn't started.

Privacy is the moat. The brain is the product.
Everything you need to use TraceMind runs on this machine.

`~/.tracemind/` is the only state. The user can `rm -rf` it.
That's the whole pitch.

— Aaditya

---

## Appendix — proof points

- **Code:** 17-crate Rust workspace, 163 tests passing on `main`.
- **Eval:** LoCoMo mini F1 25.70 (BGE), CI-gated.
- **Docs:** four canonical, all current
  - `docs/PHASE4_DELIGHT.md` — current progress + cognition roadmap
  - `docs/INTENT_SYSTEM.md` — wedge spec + 7-sprint plan
  - `docs/BRAIN_ARCHITECTURE.md` — full architecture
  - `docs/LOCOMO_RESULTS.md` — eval baselines
- **Demo:** 6-minute screen recording in
  `demo/recordings/walkthrough-clean-20260506-215825.mov`
- **Repo:** github.com/aadi2vec/tracemind (private; ask for access)
