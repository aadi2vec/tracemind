# Feedback-Driven Self-Improvement Loop

Status: design memo (2026-05-10) — pre-implementation. The retrieval
stack already accepts a **negative-feedback signal** (Sprint C-0.7);
this memo specifies the **positive signal**, the **eval harness**, the
**parameter surface**, and the **optimizer loop** that together turn
TraceMind into a system that gets *measurably* better the more you use
it — without ever phoning home.

---

## 1. Why this is the wedge, not a side quest

Every memory product on the market gets better by **scaling capture**.
We get better by **scaling feedback**. The reasons are simple:

1. **Capture is bounded by user life**, not by engineering.
   We can't make a single user write more than they already write.
2. **Feedback is unbounded** and currently entirely thrown away.
   Every time the user re-asks the same question, reformulates,
   clicks the "wrong" result, or abandons mid-query, that is a
   training signal. We log all of it (`traces.jsonl`,
   `trajectory_store`, `recent.jsonl`) and use *none* of it to
   change behaviour beyond the immediate bandit reward.
3. **Local-only makes this defensible.** Cloud competitors can't
   personalise without uploading the feedback corpus; we can.
   Their model is one global model; ours is a population of
   per-user pareto-frontier models, none of which leave the device.

This is the same insight DSPy MIPRO and GEPA bake into prompt
optimisation, ported into a retrieval-OS context.

---

## 2. The mental model: GEPA + MIPRO, adapted

### 2.1 What DSPy MIPRO does
MIPRO (Multi-prompt Instruction Proposal Optimizer) jointly searches
over **(instruction text, few-shot demonstrations)** pairs to maximise
a scalar metric on a small labeled training set. Bayesian-style
acquisition over a candidate pool.

### 2.2 What GEPA does
GEPA (Genetic-Pareto) maintains a **frontier** of program candidates
rather than collapsing to a single best. Mutations spawn children;
the frontier keeps every candidate that dominates on some metric
axis. The output is a *menu* of programs, each best on a different
query class.

### 2.3 TraceMind adaptation
We treat the *retrieval+answer stack* as the "program":

| DSPy concept            | TraceMind analog                                        |
|-------------------------|---------------------------------------------------------|
| Program parameters      | Planner thresholds, rerank α, RRA weights, MMR λ, arm-selection priors, extractor templates |
| Few-shot demonstrations | Per-context curated query→answer exemplars             |
| Metric                  | F1 + EM (offline) **and** engagement reward (online)   |
| Training set            | Replayable query trace + feedback corpus               |
| Optimizer               | `tm-tune` nightly sidecar — bayesian + genetic-pareto  |
| Output                  | A *named* config in `~/.tracemind/configs/<id>.toml`, optionally promoted to `active.toml` |

The crucial twist: **the training set never leaves the device**.
The optimizer runs in a background process when the laptop is idle
(`pmset -g batt | grep -q "AC Power"` && load < threshold).

---

## 3. What we have today vs what's missing

### 3.1 Already shipping
- `traces.jsonl` — every query + ingest with content hashes, entity IDs, arm chosen, latency.
- `trajectory_store` (`tm-episodic`) — per-query trajectory: which arm, which planner action, which entities loaded.
- `negative_signals` table — user-filed retraction with weight, kind, query_id, optional context A/B (Sprint C-0.7).
- `bandit.json` + `linucb.json` — running UCB1 + LinUCB state, already learning from rewards.
- `recent.jsonl` — capture ring buffer (engagement adjacent).
- Tier-0 extractive backend — templated synthesis, deterministic, replayable.

### 3.2 Gaps the loop needs
1. **Positive signals**. Today's reward is engagement-derived (click, re-query latency). There is no explicit "this answer was right" channel. → `positive_signals` table + `tracemind helpful <query_id> [--weight w]` CLI.
2. **Parameter surface**. The tunables live as inline constants in `tm-retrieval`, `tm-rerank`, `tm-answer`. Each needs to move into a named struct loaded from disk. → `tm-types::RetrievalParams` + extension.
3. **Eval harness**. The LoCoMo bench exists for offline F1 but is not wired to the user's feedback corpus. → `tm-eval` crate that replays cached (query, feedback) pairs against a candidate config.
4. **Optimizer**. → `tm-tune` crate — sidecar binary.
5. **Promote / rollback**. Versioned configs + a one-line rollback (`tracemind config rollback`).

---

## 4. Architecture sketch

```
                   ┌──────────────────────┐
                   │  ~/.tracemind/        │
                   │   ├─ traces.jsonl     │  (existing)
                   │   ├─ trajectories/    │  (existing)
                   │   ├─ negative_signals │  (existing C-0.7)
                   │   ├─ positive_signals │  ← F-1 (new table)
                   │   └─ configs/          │  ← F-3
                   │      ├─ active.toml     │
                   │      ├─ 2026-05-10-a.toml
                   │      └─ frontier.json  │  ← pareto candidates
                   └──────────┬───────────┘
                              │
       ┌──────────────────────┼─────────────────────────────┐
       │                      │                              │
       ▼                      ▼                              ▼
┌──────────────┐    ┌────────────────────┐         ┌────────────────────┐
│ tm-retrieval │    │ tm-eval (new)       │         │ tm-tune (new)       │
│ reads active │    │ replays query+      │         │ idle-time sidecar.  │
│ config; logs │    │ feedback against    │         │ proposes mutations  │
│ to traces +  │    │ a candidate config; │         │ + few-shot demos;   │
│ trajectories │    │ returns F1/EM +     │         │ runs tm-eval; keeps │
│              │    │ engagement metrics  │         │ pareto frontier;    │
│              │    │                     │         │ promotes winners    │
└──────────────┘    └────────────────────┘         └────────────────────┘
```

No new network paths. Everything is files + processes on one machine.

---

## 5. The parameter surface — what we actually tune

This is the "program" the optimizer evolves. We start small (15–20
knobs) and grow.

### 5.1 Planner (`tm-retrieval::QueryPlanner`)
- `temporal_token_threshold` — confidence at which "last week" forces a temporal route.
- `decompose_complexity_floor` — minimum compound-query score to invoke decomposition.
- `analogy_token_overlap_min` — when to short-circuit to the analogy solver.

### 5.2 Bandit (`tm-controller`)
- `ucb_c` — UCB1 exploration constant (currently hardcoded √2).
- `linucb_alpha` — LinUCB exploration radius.
- `arm_prior_mean[5]` — initial reward prior per arm (start uniform; learn per-user).

### 5.3 Retrieval pipeline (`tm-retrieval::engine`)
- `top_k_multiplier` — over-fetch ratio when reranker is present.
- `rerank_alpha` — ColBERT vs vector blend weight.
- `rra_value_weight, rra_freq_weight, rra_recency_weight` — RRA fusion mix.
- `mmr_lambda` — diversity vs relevance in entity selection.
- `cross_context_penalty` — soft penalty applied to foreign-context candidates (currently -0.15, hardcoded — first thing we want learned per-user).
- `low_confidence_threshold` — when to emit "I don't know" instead of stretching.

### 5.4 Answer (`tm-answer`)
- `extractive_template_set` — which templated patterns are active for each `TaskKind` (single_hop, multi_hop, temporal, ...).
- `tier1_few_shot_examples` — when Tier-1 LLM is enabled, the in-context demos prepended to each prompt. **This is the MIPRO entry point.**
- `tier_fallback_order` — preferred tier sequence per task.

---

## 6. Optimizer algorithm (GEPA-lite, on-device)

### 6.1 Inputs
- `corpus = (replayable queries + their labeled outcomes)` derived
  from `traces.jsonl` × `{positive_signals, negative_signals}`.
- `seed_config = active.toml`.
- `budget = N minutes idle CPU + M MB RAM` (default: 10 min, 200 MB).
- `query_classes = {single_hop, multi_hop, temporal, cross_context, low_confidence}`.

### 6.2 Loop
```
frontier = {seed_config}
for generation in 1..G:
    parent = sample_from_frontier(weighted_by_dominance)
    child = mutate(parent)         # see 6.3
    scores = tm_eval(child, corpus)
    if dominates_any_axis(scores, frontier):
        insert_into_frontier(child, scores)
        evict_dominated(frontier)
    if budget_exhausted: break
write_frontier(~/.tracemind/configs/frontier.json)
```

### 6.3 Mutation menu
- **Numeric jitter** — Gaussian perturbation on continuous params (rerank α, MMR λ).
- **Threshold step** — ±1 unit on integer knobs (top_k, hop_count).
- **Template swap** — replace one extractor template with a sibling from the same TaskKind.
- **Demo curation** — for Tier-1: pick a different (query, answer) pair from the user's high-positive corpus as a few-shot example.
- **Feature toggle** — flip a boolean knob (e.g., `enable_pagerank_blend`).

### 6.4 Promotion policy
The user is *never* surprised. The optimizer produces a *frontier* but
does not auto-promote. Surfacing options:
1. **Conservative** (default): `tracemind config status` shows
   "3 candidates pareto-dominate active on cross-context F1; review
   with `tracemind config diff <id>`."
2. **Opt-in auto-promote**: a flag `auto_promote = "if_dominates_on_all_axes"`.
3. **Rollback**: `tracemind config rollback` reverts to the previous
   `active.toml` instantly (atomic symlink swap).

---

## 7. Building the corpus

A retrieval system has no labels by default. We synthesise them:

| Source signal                  | Becomes                                          |
|--------------------------------|--------------------------------------------------|
| `not-related` row              | Negative example: (query, result_id) ≠ relevant  |
| `helpful` row (F-1, new)       | Positive example: (query, result_id) = relevant  |
| Rapid re-query of same text    | Implicit negative on the prior arm + result set  |
| Re-query with reformulated text | Implicit negative + suggests a query-rewrite demo |
| Click + dwell ≥ 5s on a result | Implicit positive on that result_id              |
| Manual `feedback <arm> <reward>` | Direct arm-level label                         |
| Commitment outcome (Phase 4)   | Long-horizon label: the prediction the system made about an intent, did it pan out? |

The corpus is *never* uploaded. The optimizer reads it directly from
the user's own SQLite + JSONL.

---

## 8. Why MIPRO-style few-shot demo selection matters for Tier-1

Once `local-llm` feature is on (Qwen 2.5 1.5B Q4_K_M), the Tier-1
backend runs a prompt of the form:

```
You are TraceMind, a local memory assistant. Use only the
following grounded context.

[3 few-shot examples chosen by the optimizer]

Context: {triples + raw spans}
Question: {user question}
Answer:
```

The few-shot examples are **the strongest lever** we have on Tier-1
quality, by a wide margin. They should be:

1. **Drawn from the user's own positive feedback corpus.** A
   structurally similar (query, answer) pair the user already
   confirmed as correct teaches Tier-1 the user's style, granularity,
   and domain better than any generic example.
2. **Selected per query class.** A temporal question gets temporal
   demos; a cross-context retraction gets retraction demos.
3. **Optimised, not stapled on.** MIPRO-style — propose, evaluate,
   keep, mutate.

This is the difference between "we run a local LLM" (commodity) and
"we run a local LLM that *learned how to answer **you** specifically*"
(defensible).

---

## 9. Sprint plan

Sequential dependencies are real: F-1 → F-2 → F-3 → F-4 → F-5.

- **F-1 Positive signal CLI + storage.** New table
  `positive_signals(query_id, result_id, weight, kind, ts)`, mirror
  of `negative_signals`. New CLI: `tracemind helpful <query_id>
  [<result_id>] [--weight w] [--kind k]`. MCP equivalent
  `memory_feedback {kind: "helpful"}`. ~1 day.
- **F-2 Corpus extractor.** `tm-eval::corpus::build()` reads traces
  + positive + negative signals, materialises a typed
  `EvalCorpus { examples: Vec<EvalExample> }`. Tests: idempotent,
  excludes ingest traces, dedupes. ~1 day.
- **F-3 Parameter surface.** Move ~20 named knobs into
  `tm-types::TuneConfig`, load from
  `~/.tracemind/configs/active.toml` (default file shipped with the
  release). All call sites read from this struct. CLI:
  `tracemind config show / edit / rollback / diff`. ~2 days.
- **F-4 `tm-eval` crate.** Replays an `EvalCorpus` against a given
  `TuneConfig`, returns per-axis scores `EvalReport { f1, em,
  engagement_reward, neg_signal_rate, latency_p50, latency_p95 }`.
  ~1.5 days.
- **F-5 `tm-tune` crate (GEPA-lite).** Idle-time binary. Loop in §6.2.
  Pareto frontier persisted to `frontier.json`. CLI:
  `tracemind tune run [--budget-min 10]`,
  `tracemind tune frontier`, `tracemind tune promote <id>`. ~3 days.
- **F-6 MIPRO few-shot demo selection (Tier-1 gated).** When
  `local-llm` is built and a Tier-1 backend exists, extend the
  mutator with "demo swap" actions; tune the (instruction, demos)
  jointly. ~2 days, blocked on Tier-1 wiring.

Total ~10 working days; F-1..F-3 unlocks the "feedback is a feature"
demo even without the optimizer.

---

## 10. What this lets the demo say

Today's demo says: "We have local memory."
Post-F-1..F-3 demo says: "When you tell us we got it wrong, the
system *retrains* — and you can roll back."
Post-F-5 demo says: "Your laptop spent 10 minutes overnight making
**your** TraceMind 4% better at the kind of questions **you** ask."
Post-F-6 demo says: "Your local LLM picked **your** prior answers
as its own few-shot demos. It is now literally writing in your
voice, without ever sending a token to a cloud."

That is the loop. That is the moat.

---

## 11. Risks & non-goals

- **Not a fine-tune of the embedder.** We do not retrain BGE-small
  on-device — too expensive for our footprint budget, and the
  QLoRA personalization track in `tracemind_llm_finetune_direction`
  handles weight updates separately. `tm-tune` only moves *config*.
- **Not a global LM optimizer.** No prompt-search across other users.
  Per-device, per-user, period.
- **Pareto frontier can drift.** A bad mutation that accidentally
  dominates on a thin axis can crowd the frontier. Mitigation: a
  minimum-support floor — a candidate must beat seed on ≥ K examples
  per axis to enter the frontier.
- **User confusion.** Auto-promote is **off by default.** Every
  change is reviewable, every change is reversible.
