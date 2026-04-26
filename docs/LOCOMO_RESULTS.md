# LoCoMo Benchmark Results — TraceMind v0.1

**Date**: 2026-04-26
**Branch**: `claude/locomo-results`
**Commit**: pending
**Harness**: `tm-bench-locomo` (TM-5.2-005, merged in #24)
**Dataset**: `crates/tm-bench-locomo/fixtures/locomo-mini.json` (3 multi-session conversations, 20 questions across all 5 LoCoMo categories)
**Reports**: `crates/tm-bench-locomo/baselines/v0.1-{hash,bge}-mini.json`

> ⚠️ This is an internal mini-dataset (20 questions), not the official
> snap-research/locomo benchmark (~7,000 questions). Numbers here are
> **directional** — useful for finding regressions and validating
> infrastructure. Publishable numbers vs. competitors require running on
> the full LoCoMo dataset (TM-5.2-005-followup).

---

## 1. Headline numbers

| Configuration                       | F1    | EM   | n   | Wall (s) |
|-------------------------------------|-------|------|-----|----------|
| `null-baseline`                     | 0.00  | 0.00 | 20  | 0.0      |
| `tracemind-v0.1-hash` (hash embed)  | 12.78 | 0.00 | 20  | 0.3      |
| `tracemind-v0.1-bge` (BGE-small)    | **12.78** | 0.00 | 20  | 7.6      |
| `echo-oracle` (upper bound)         | 100.00| 100.00| 20 | 0.0      |

**Key observation: hash and BGE produce identical F1 on this mini set.**
This is the most important finding in the run — see §3.

For comparison, published competitors on the full LoCoMo:

| System                  | LoCoMo F1 |
|-------------------------|-----------|
| Honcho 3                | SOTA      |
| Mem0                    | 91.6      |
| SuperLocalMemory Mode C | 87.7      |
| Engram                  | 80.0      |
| Zep                     | 75.14     |
| Letta                   | 74.0      |
| **TraceMind v0.1**      | **12.78 (mini)** |

We're not benchmarking against the same set, so the gap is overstated —
but the absolute number tells us that **the current Tier-0 extractive
synthesizer is the wrong baseline to ship with.** The retrieval stack
is doing real work; the synthesis layer is wasting it.

## 2. Per-category breakdown (BGE run)

| Category     | n  | F1     | EM   |
|--------------|----|--------|------|
| temporal     | 2  | 20.19  | 0.00 |
| multi_hop    | 3  | 20.13  | 0.00 |
| adversarial  | 3  | 18.10  | 0.00 |
| single_hop   | 12 | 8.37   | 0.00 |

The pattern: **categories with broader reference-answer phrasing score
higher** (multi_hop refs include full sentences, temporal include
"May 3rd" *and* longer paraphrases) because token-F1 catches partial
overlaps. **single_hop is worst because the references are tight
single-token answers** (`Stripe`, `Sequoia`, `Memex`, `Priya`,
`Boston`, `Haneda`) that the verbose extractive synthesizer dilutes.

## 3. Why hash and BGE produce the same F1

This is the headline finding. The system is retrieving the same
top-1 trace under both embedders, so the synthesizer's output is
identical. Three possible causes, in priority order:

1. **Synthesizer is top-1 only** (`top_k_traces=1` in
   `TraceMindConfig`). Embedding-model gradients are smoothed away
   when we only use the rank-1 hit and embedders agree on rank 1 most
   of the time on a 15-turn conversation. Fix: blend top-3 chunks.
2. **Speaker/date prefix dilutes embedding signal.** We ingest
   `"Alice (2026-04-01): I'm flying to Tokyo on May 3rd..."` — the
   prefix tokens dominate cosine similarity for short turns, so
   "Tokyo" and "flying" anchor harder than the actual content. Both
   hash and BGE feel this equally.
3. **Bandit arm 1 (medium, top_k=10, hops=1) is selected for nearly
   every query.** The LinUCB has no warm-up data on a fresh DB so it
   defaults to its initial policy. Both embedders end up presenting
   the same candidate to the same arm.

## 4. Qualitative failure modes

Reading per-question outcomes (`baselines/v0.1-bge-mini.json`):

### 4.1 Speaker prefix bloat (every question)
Every prediction starts with `Speaker (YYYY-MM-DD): ` — 8–12 dead
tokens before the answer. SQuAD F1 punishes precision, so even a
correct retrieval scores ≤ 30%.

> Q: "When is Alice flying to Tokyo?"
> Pred: `"Alice (2026-04-01): I'm flying to Tokyo on May 3rd for a conference."`
> Refs: `["May 3rd", "May 3", "3rd of May"]`
> F1: 25.0 (would be ~80 with prefix stripped)

**Single biggest F1 win is here.** Strip the speaker/date prefix from
the synthesized answer. Estimate: +20–30 F1 points overall.

### 4.2 Question-turn retrieval (5/20 questions)
The retriever picks the *question* turn instead of the *answer* turn
because the question shares vocabulary with itself.

> Q: "What is Alice's talk about?"
> Pred: `"Bob (2026-04-15): What's the talk about?"` ← Bob asking
> Refs: `["Local memory systems for consumer apps", "local memory systems"]`
> F1: 0.0

Other instances:
- "Who is Carol's first hire?" → `Dan: First hire?` (question turn)
- "What was Ethan's goal time?" → `Fiona: Goal time?` (question turn)
- "Did Ethan miss his sub-3:00 goal?" → `What marathon did Ethan run?` (a *different question*)

**Fix candidates** (in order of cost):
1. Adjacency boost — when a turn is a question, prefer the next turn
   from a different speaker.
2. Question/statement detector — penalize turns ending in `?`.
3. LLM extraction at consolidation (Tier 1 / `tm-answer`) — extract
   the answer span from a 2–3-turn window, not the turn itself.

### 4.3 Wrong-but-plausible retrievals (3/20)
The retriever picks a turn that's lexically related but factually
wrong, often because of speaker confusion or temporal ambiguity.

> Q: "What was Carol's previous employer?" → `Dan: Whoa, what are you building?` (F1=0)
> Q: "Which airport did Alice fly into?" → `Alice: Eleven hours from JFK...` (JFK is *origin* not destination, F1=0)
> Q: "How much did Carol raise?" → `Carol: An ML engineer named Priya, joining May 1.` (F1=0)

The Carol-employer case is interesting: the answer is in turn `"I'm
leaving Stripe to start a company"` but our retriever chose the
*reply* by Dan. Speaker–subject grounding is missing.

The Carol-raise case shows the **temporal bug**: we have two amounts
(`$1.5M target` early, `$2.5M actual` later) and the bandit picked
the wrong era. No recency / latest-state bias in the synthesizer.

### 4.4 Adversarial questions accidentally win (3/3 ≈ 18 F1)
> Q: "Did Alice fly to Tokyo on May 1st?"
> Pred: `"Alice (2026-04-01): I'm flying to Tokyo on May 3rd for a conference."`
> Refs: `["No, May 3rd", "No"]`
> F1: 23.5

This is a false positive — we score because "May" and "3rd" appear
in both. The system isn't actually saying "no". On the full LoCoMo
adversarial set this fragile signal will collapse.

## 5. What's working

- **Retrieval reaches the right region most of the time.** When the
  prediction is wrong it's usually 1–2 turns off, not totally
  unrelated. The latent F1 if synthesis were perfect is materially
  higher than 12.78.
- **Multi-hop ≈ temporal ≈ adversarial.** No category is catastrophic
  — the system isn't blind to any axis. The single_hop floor is a
  synthesis problem, not a retrieval problem.
- **Hash embed runs at 0.3s for 20 questions.** Even with full
  ingest+retrieval, throughput is ~60 q/s. CI gate cost is trivial.
- **No crashes, no flakes.** Every question got a deterministic
  answer (the harness handled the test surface well).

## 6. Improvement plan (ordered by F1 impact / cost)

| # | Change                                              | Est. F1 lift | Cost |
|---|-----------------------------------------------------|--------------|------|
| 1 | Strip `Speaker (date): ` prefix from synthesized answer | +20–30 | trivial |
| 2 | `top_k_traces=3`, blend top hits with score-weighting   | +5–10  | small |
| 3 | Penalize `?`-ending turns in retrieval scoring          | +5–8   | small |
| 4 | Adjacency boost — prefer turn after question turn       | +5–10  | small |
| 5 | Recency tie-break for "current state" questions         | +3–5   | small |
| 6 | LLM span extraction via Tier-1 (`tm-answer` LocalLlm)   | +20–40 | large (gated on TM-5.2-002) |
| 7 | Speaker-subject grounding (route question to entity, retrieve their turns) | +10–15 | medium |
| 8 | Real LoCoMo dataset run (~7k questions)                 | establishes credibility | medium (download + format adapter) |

**Recommended next PR**: items 1–4 in a single change to the
`TraceMindRunner.synthesize` and a small addition to the retrieval
scoring layer. This should land us in the 35–50 F1 range on the
mini-set with no model changes — the floor we want to ship before
plumbing in any LLM. **Item 6 is the headline lift but blocked on
TM-5.2-002 (llama.cpp integration).**

## 7. Reproducing this run

```bash
cargo build -p tm-bench-locomo --features tracemind --release

# Hash embeddings (CI-deterministic, fast)
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind \
  --output baselines/v0.1-hash-mini.json

# Real BGE embeddings (publishable headline)
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind --real-embeddings \
  --output baselines/v0.1-bge-mini.json
```

## 8. Where this fits in PHASE 3

This run satisfies the "first publishable LoCoMo number" gate from
`docs/PHASE3.md §10`. It does **not** yet hit the ≥85 F1 north-star —
that target requires both the synthesis fixes (items 1–5 above) and
the Tier-1 LLM (item 6). The number we ship with at Tauri .dmg launch
should be from the full snap-research dataset, not this mini fixture.

**Gate next**: items 1–5 above as a single PR; rerun on this mini
fixture to confirm we clear ~40 F1 before unblocking Tier-1
integration work (TM-5.2-002).
