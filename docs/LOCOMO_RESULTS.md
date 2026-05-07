# LoCoMo Benchmark Results — TraceMind

**Date**: 2026-04-26
**Branch**: `claude/locomo-results`
**Harness**: `tm-bench-locomo` (TM-5.2-005, merged in #24)
**Dataset**: `crates/tm-bench-locomo/fixtures/locomo-mini.json` (3 multi-session conversations, 20 questions across all 5 LoCoMo categories)
**Reports**: `crates/tm-bench-locomo/baselines/v0.{1,2}-{hash,bge}-mini.json`

> ⚠️ This is an internal mini-dataset (20 questions), not the official
> snap-research/locomo benchmark (~7,000 questions). Numbers here are
> **directional** — useful for finding regressions and validating
> infrastructure. Publishable numbers vs. competitors require running on
> the full LoCoMo dataset (TM-5.2-005-followup).

---

## 1. Headline numbers

### v0.2 (current — synthesis fixes applied)

| Configuration                       | F1    | EM   | n   | Wall (s) |
|-------------------------------------|-------|------|-----|----------|
| `null-baseline`                     | 0.00  | 0.00 | 20  | 0.0      |
| `tracemind-v0.2-hash` (hash embed)  | **25.70** | 5.00 | 20  | 0.3      |
| `tracemind-v0.2-bge` (BGE-small)    | **25.70** | 5.00 | 20  | 4.7      |
| `echo-oracle` (upper bound)         | 100.00| 100.00| 20 | 0.0      |

**Δ from v0.1**: +12.92 F1, +5.00 EM. Doubled the headline number with
zero changes to retrieval — all gain came from the synthesis layer.

### v0.1 (baseline before fixes)

| Configuration                       | F1    | EM   |
|-------------------------------------|-------|------|
| `tracemind-v0.1-hash`               | 12.78 | 0.00 |
| `tracemind-v0.1-bge`                | 12.78 | 0.00 |

For comparison, published competitors on the full LoCoMo:

| System                  | LoCoMo F1 | Local LLM? |
|-------------------------|-----------|------------|
| Mem0                    | 91.6      | No (cloud) |
| SuperLocalMemory Mode C | 87.7      | Yes        |
| Engram                  | 80.0      | Yes        |
| Zep                     | 75.14     | No (cloud) |
| Letta                   | 74.0      | No (cloud) |
| **TraceMind v0.2 (mini)**| **25.70** | **No (Tier-0 extractive)** |

The gap is overstated because we're on a 20-question mini-set, not the
~7k-question official LoCoMo. The absolute number tells us we're roughly
**a third of the way to a publishable single-digit-LLM-free baseline**.

## 2. Per-category breakdown (BGE v0.2)

| Category     | n  | F1     | EM   | v0.1 F1 | Δ      |
|--------------|----|--------|------|---------|--------|
| multi_hop    | 3  | 36.57  | 0.00 | 20.13   | +16.4  |
| adversarial  | 3  | 25.07  | 0.00 | 18.10   | +7.0   |
| single_hop   | 12 | 24.64  | 8.33 | 8.37    | +16.3  |
| temporal     | 2  | 16.67  | 0.00 | 20.19   | −3.5   |

**Single-hop more than tripled** (8.37 → 24.64). The big lever was the
speaker-prefix strip — most single-hop refs were tight tokens (`Memex`,
`Priya`, `Hanson`) that the prefix used to drown out.

**Temporal regressed slightly** (-3.5 F1). One question
("When did Ethan run Boston?") returned `Finished Boston in 2:58:42!`
instead of `April 20`. We don't have date-aware extraction in Tier 0.

## 3. What changed in v0.2 (in `tracemind_runner.rs`)

All four fixes from the v0.1 analysis landed:

1. **Strip `Speaker (date): ` prefix at synthesis** — heuristic strip of
   the first `: ` separator if the head is short and contains no
   terminal punctuation. Single biggest lift.
2. **Q→A adjacency** — when the best lexical match for the user
   question is itself a question turn, return the next ingested turn
   (the answering turn). Worked perfectly on
   "What is Alice's talk about?" (0 → 100 F1).
3. **Skip question turns in fallback** — never emit a `?`-ending
   prediction; always step to the next non-question turn.
4. **Strip speaker prefix when scoring overlap** — otherwise every Carol
   turn scored +1 just because the question contained "carol". Removing
   the speaker name from the matchable string forced the scorer to use
   actual content tokens.

**Bonus discovery**: the `RetrievalEngine` returns `signal_hits=0`,
`traces=0`, `entities=0` for almost every query on this fixture (cosine
threshold 0.4 on 15-turn cold-start conversations is too strict). The
v0.2 synthesizer is therefore using `substring_fallback` as the
*primary* retrieval path, not a fallback. The engine doesn't yet
contribute to scores on the mini-set.

This is itself an actionable finding — see §6.

## 4. Per-question outcomes (v0.2 BGE)

| F1 | Q | Pred | Ref |
|----|---|------|-----|
| 33.3 | When is Alice flying to Tokyo? | I'm flying to Tokyo on May 3rd for a conference. | May 3rd |
| **100** | What is Alice's talk about? | Local memory systems for consumer apps. | Local memory systems for consumer apps |
| 18.2 | Why is Alice going to Tokyo? | I'm flying to Tokyo on May 3rd for a conference. | To present at NeurIPS regional |
| 46.2 | Where is Alice staying in Tokyo? | No, this time I booked a hotel in Shinjuku near the venue. | Shinjuku |
| 0 | Which airport did Alice fly into? | I'm flying to Tokyo on May 3rd for a conference. | Haneda |
| 30.8 | Did Alice fly to Tokyo on May 1st? | I'm flying to Tokyo on May 3rd for a conference. | No, May 3rd |
| 0 | What was Carol's previous employer? | (empty) | Stripe |
| 0 | Who led Carol's pre-seed round? | Pre-seed, $1.5M target. | Sequoia |
| 0 | How much did Carol raise? | (empty) | $2.5M |
| 20 | What did Carol rename her company to? | Renamed Loom to Memex — the original name was trademarked. | Memex |
| 61.5 | Why did Carol rename the company? | Renamed Loom to Memex — the original name was trademarked. | Loom was trademarked |
| 25 | Who is Carol's first hire? | An ML engineer named Priya, joining May 1. | Priya |
| 44.4 | Did Carol raise from Andreessen Horowitz? | Got the term sheet from Sequoia today. | No, from Sequoia |
| 18.2 | What marathon did Ethan run? | Did a 22-mile long run today at marathon pace. | Boston |
| 50 | What was Ethan's goal time? | Sub-3:00 is the dream, but realistically 3:10. | sub-3:00 |
| 0 | What was Ethan's finish time? | Sub-3:00 is the dream, but realistically 3:10. | 2:58:42 |
| 30 | Did Ethan beat his goal? | Sub-3:00 is the dream, but realistically 3:10. | Yes |
| 0 | When did Ethan run Boston? | Finished Boston in 2:58:42! | April 20 |
| 36.4 | Which training method did Ethan use? | Hanson method, 6 days a week, peaking at 60 miles. | Hanson |
| 0 | Did Ethan miss his sub-3:00 goal? | Sub-3:00 is the dream, but realistically 3:10. | No, he ran 2:58:42 |

## 5. Remaining failure modes

The remaining 0-F1 questions (7/20) cluster into four buckets:

### 5.1 Yes/No answers we never produce (3/20)
- "Did Ethan beat his goal?" → ref `Yes`
- "Did Ethan miss his sub-3:00 goal?" → ref `No, he ran 2:58:42`
- "Did Carol raise from Andreessen Horowitz?" → ref `No, from Sequoia`
  (this one scores 44 because "from Sequoia" is in the prediction)

These need *yes/no inference*, not extraction. Adversarial questions
require the system to *contradict* the question. Tier-0 extractive
cannot do this. **Tier-1 LLM is the only path to >50 F1 here.**

### 5.2 Single-token specific answers (3/20)
- "Stripe", "$2.5M", "Haneda", "April 20"

The right turn was probably retrieved but the surrounding text dilutes
the precise span. We'd need *span extraction* — pick "Stripe" out of
"I'm leaving Stripe to start a company". Tier-0 has no NER for that
beyond what `tm-ingest` already extracts.

### 5.3 Tied retrieval picks the wrong era (2/20)
- "Who led Carol's pre-seed round?" → Pre-seed target turn instead of
  Sequoia term-sheet turn
- "How much did Carol raise?" → empty (no overlap with `$2.5M`)

These are recency/state-merging problems. Tier-0 can't reason about
"latest known value".

### 5.4 Question→answer adjacency picked the wrong neighbour (1/20)
- "What was Ethan's finish time?" → "Sub-3:00 is the dream..." instead
  of "Finished Boston in 2:58:42!"

The question turn matched lexically, but the *answer* came two turns
later, not one. Need bigger window.

## 6. How far are we from acceptable performance without a local LLM?

This is the question that matters for shipping. **My answer: we can
realistically clear 45–55 F1 on the mini-set without an LLM, but the
ceiling is somewhere around 60.** Everything above ~60 F1 requires
either an LLM or a fundamentally different retrieval architecture.

### Path to 45–55 F1 (Tier-0 only)

| # | Change | Est. F1 lift | Cost |
|---|--------|--------------|------|
| 1 | Lower `signal_search` cosine threshold from 0.4 → 0.2 so the actual retrieval engine produces hits on cold conversations | +5–10 | trivial (one-line) |
| 2 | Window the Q→A adjacency to next 1–3 turns (not just next turn) — fixes "Ethan's finish time" | +3–5 | small |
| 3 | Heuristic NER for the four LoCoMo answer types: dates, money, named entities, percentages — extract spans rather than full sentences | +8–15 | medium |
| 4 | Recency bias: when multiple turns share the same entity (Carol → company), prefer the latest mention | +3–5 | small |
| 5 | Yes/no oracle: pre-screen the question for `Did/Was/Is` patterns; emit `No` if the answer-turn predicate disagrees with the question's polarity | +5–10 | medium |
| 6 | Real LoCoMo dataset run (~7k questions) for credibility — likely *lower* F1 than mini, but is the publishable number | establishes credibility | medium |

**1+2+3+4** without the LLM should get us to ~45 F1 on the mini.
Adding **5** (a careful yes/no oracle) plausibly takes us to ~55.

### What the LLM unlocks (Tier-1)

| Failure bucket | Tier-0 ceiling | Tier-1 ceiling |
|----------------|---------------|----------------|
| Span extraction (Stripe, Haneda) | ~40 F1 | ~85 F1 |
| Yes/No adversarial | ~30 F1 | ~80 F1 |
| Multi-turn synthesis | ~50 F1 | ~85 F1 |
| Recency / state-merging | ~40 F1 | ~75 F1 |

The published 80–90 F1 systems (Mem0, SuperLocalMemory, Engram) all use
LLMs for synthesis. **There is no published Tier-0-only system that
clears 60 F1 on full LoCoMo.** Our north-star of ≥85 F1 requires
Tier-1, full stop.

### Recommendation

Ship v0.2 (25.7 F1 mini) **as the regression-gate baseline**. Land
fixes 1–4 above as a single PR to push mini-F1 to ~45 — that becomes
the "Tier-0 acceptable bar". Then unblock TM-5.2-002 (llama.cpp
integration) as the path to 70+. **Do not ship a public benchmark
number until we run on the full LoCoMo dataset** — the mini is a
regression gate, not a marketing artifact.

## 7. Reproducing this run

```bash
cargo build -p tm-bench-locomo --features tracemind --release

# Hash embeddings (CI-deterministic, fast)
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind \
  --output crates/tm-bench-locomo/baselines/v0.2-hash-mini.json

# Real BGE embeddings (publishable headline)
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind --real-embeddings \
  --output crates/tm-bench-locomo/baselines/v0.2-bge-mini.json

# Regression gate vs. v0.2
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind --real-embeddings \
  --gate-against crates/tm-bench-locomo/baselines/v0.2-bge-mini.json \
  --tolerance 0.5
```

## 8. Where this fits in the roadmap

This run is the second milestone for the LoCoMo quality gate (now
tracked in `docs/PHASE4_DELIGHT.md`). v0.1 set the floor (12.78); v0.2
shows the floor was synthesis, not retrieval. The ≥85 F1 north-star
remains gated on Tier-1 (TM-5.2-002), but the next intermediate gate
should be **45 F1 on the mini fixture, Tier-0 only**, achievable from
the changes in §6 without any LLM work.
