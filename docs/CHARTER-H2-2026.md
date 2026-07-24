# TraceMind — H2 2026 Charter (Jul–Dec)

**Effective:** 2026-07-07
**Status:** CANONICAL for H2 2026 planning. Supersedes `docs/PROJECT_2026.md` for H2. Wedge concepts from prior sprint memos ([[tracemind-project-2026]], [[tracemind-strategic-direction]], [[tracemind-context-graph-direction]], [[tracemind-event-graph-pivot]], [[tracemind-composition-layer]], [[tracemind-evg-ont-lgm-order]]) are retained as lineage — they still define what we're building. This document redefines *how*.

---

## 1. Thesis

TraceMind is a **local, self-improving memory OS** that keeps a user's context under their own control, composes it across every AI conversation they have, and gets measurably better at their real workflow with no code deploy in between.

The wedge has not changed: **Commitment → Outcome ledger**, **event graph over entity-SVO**, **composition layer as graph algebra**, **host-agnostic MCP**, **user-owned graph**. The mechanism to deliver that wedge has.

## 2. What Changed (Jul 2026)

In a six-week window:

- **AIE World's Fair 2026** — Anthropic, DeepMind, Amazon keynotes on recursive self-improvement, autoresearch loops (Generator / Verifier / Updater), memory-as-continual-learning.
- **GEPA (Genetic-Pareto Prompt Evolution)** — reflective mutation + Pareto archive beats GRPO with 35× fewer rollouts, no weight updates. Obsoletes LoRA for retrieval-policy personalization.
- **Superlinked Spaces** — the "Mixture of Encoders" pattern: multiple embedding spaces, query-time weight vectors, composed retrieval. Portable as a pattern (not the SDK) into Rust.
- **NirDiamant 30-technique catalog** — hierarchical memory tiers, memory routing (should we retrieve at all?), memory-as-tools verbs, working-memory pins.
- **Engram Labs launch** — validates the category (cloud personalization for LLMs). Confirms our wedge is *local + user-owned + composed*, not "another memory API".
- **BGE-M3** — one model, three outputs (dense + learned sparse + multi-vector). Dense path replaces BGE-small in Q3; multi-vector path retires mxbai-colbert in Q4. Sparse path evaluated as optional. BGE-M3 is not yet in fastembed's official list — requires direct ONNX loading.

The wedge holds. The mechanism to close the self-improvement loop is now credible without a Python fine-tune sidecar.

## 3. Positioning

| Player            | Deployment      | Graph ownership | Self-improvement | Contradiction handling |
|-------------------|-----------------|-----------------|------------------|------------------------|
| Mem0              | Cloud API       | Vendor          | None (fixed)     | None                    |
| Zep / Graphiti    | Cloud + OSS     | Vendor primary  | None             | Weak                    |
| Letta (MemGPT)    | Hosted          | Vendor          | None             | None                    |
| Engram Labs       | Cloud           | Vendor          | LoRA (cloud)     | None                    |
| LangMem           | Framework       | User's DB       | None             | None                    |
| **TraceMind**     | **Local-first** | **User**        | **GEPA (Q4, building)** | **Temporal-KG (Q3.4, building)** |

**Honest state as of Jul 2026:** TraceMind's architectural advantage is the local temporal-KG substrate (bitemporal `valid_from`/`valid_to`, already shipped) and the user-owned graph. GEPA is H2's build target, not a shipped capability. Don't show this table to investors until Q3.4 (contradiction-rate metric) is published and Q3.8 GEPA spike has a result. Then the table becomes true.

TraceMind's differentiator is **the local self-improvement loop closing**, not the substrate. Ship Pillar 1 fast and publish benchmarks — that is the moat.

## 4. Non-Goals for H2

- No cloud personalization service. No hosted API. No account system.
- No QLoRA / QDoRA Python sidecar in H2. Deferred to 2027; revisit only if GEPA cannot cover a specific capability (candidate: generation style, not retrieval).
- No Foundry-style ontology UI. Ontology remains internal infra with accept/reject prompts.
- No new host integrations beyond MCP (Claude Code + Goose). Tauri stays P1, CLI stays maintenance.
- No standalone Reason navigation surface. Reasoning surfaces as verb cards in Brief + Dashboard.

## 5. Seven Pillars

### Pillar 1 — GEPA-driven self-improving retrieval policy
- LinUCB arm selection + Reflexion verbal post-mortem after every session
- GEPA reflective-mutation loop over retrieval-policy prompts and arm-weight vectors
- Pareto archive on `{F1, latency, contradiction-rate, LongMemEval-multi-hop}`
- Verifier gate: only accept mutations that pass a held-out anchor eval
- **No fine-tune, no cloud, no weight updates.** Runs nightly on-device.
- **Feasibility prerequisite (Q3 exit gate)**: before shipping `tm-gepa` in Q4, a spike must demonstrate Δ F1 > 0 on at least one synthetic GEPA mutation replayed against `~/.tracemind/traces.jsonl`. GEPA is validated in the retrieval-policy domain (not just LLM task prompts as in the original paper) before the full Q4 build commits.

### Pillar 2 — Composed multi-space retrieval (`ComposedIndex`)
Superlinked pattern ported to Rust. First-class spaces:
- **text** — BGE-M3 dense (Q3, 768d) + multi-vector/MaxSim (Q4, replaces mxbai-colbert) + sparse (Q4, optional). Single model download; three output paths phased.
- **recency** — time-decay space
- **confidence** — governance-score space
- **entity-type** — categorical space
- **signal-source** — capture-provenance space
- **host/session** — scoping space

Query-time weight vectors per verb (`recall`, `plan`, `contradict`, `reflect`). Matryoshka slicing gives free hierarchical retrieval.

### Pillar 3 — Hierarchical substrate + memory-as-tools verbs
- **hot / warm / cold tiers** in `tm-episodic` with promotion + demotion
- `tm-mcp` verbs: `memory_pin`, `memory_forget`, `memory_promote`, `memory_contradict`, `memory_reflect`
- **Memory routing gate** in `QueryPlanner`: a binary `should_retrieve: bool` decision that fires *before* LinUCB arm selection. Signals: query entropy, `host_id` policy, working-memory cache hit, and `memory_feedback` scores from Pillar 7's signal fabric. This is a routing layer, not a bandit arm — conflating the two would corrupt arm statistics with a qualitatively different decision.

### Pillar 4 — Benchmark legibility
- **LoCoMo** (already shipped) — maintain > 49 F1 gate
- **LongMemEval** — new harness, multi-hop and temporal
- **BEAM** — new harness, contradiction stress
- **Contradiction-rate** — head-to-head vs Mem0, Zep, Letta, Engram
- All reproducible in-repo. Contradiction-rate is the axis competitors can't win on architecturally (they don't retain temporal edges).

### Pillar 5 — Two-surface ship discipline
- **MCP** (Claude Code + Goose) — P0
- **Tauri Brief** — P1
- **CLI** — maintenance
- No new hosts in H2.

### Pillar 6 — Wedge continuity + user-visible narrative

The internal mechanisms (GEPA, ComposedIndex, hierarchical tiers) are invisible to the user. The user-visible story for H2 must be concrete:

> **"Your memory got better at understanding what you actually need — with no effort from you."**

This surfaces as three observable behaviors:
1. **Brief cards that match your verb rhythm** — if you `contradict` twice as often as median, the brief surfaces contradiction cards without you asking. Users notice this within 5 sessions.
2. **Composition suggestions across sessions** — "You've worked on X in three separate Claude Code sessions. Want to bridge this context?" This is the composition layer made tangible, not an algebra op the user configures.
3. **Contradiction notifications that stop repeating** — after a rollback, the same contradiction family is suppressed. Users notice the system is "listening."

These are the three measurements for "does the user feel it?" — not F1 on LongMemEval.

**Unchanged wedge commitments:**
- Commitment → Outcome ledger
- Event graph over entity-SVO
- Composition layer graph algebra `∪ ∩ \ filter() bridge()`
- EVG → ONT → LGM sprint order
- No ontology UI
- User-owned graph, local-only

### Pillar 7 — Recursive self-improvement across the whole stack

Pillar 1 makes the *retrieval policy* better. Pillar 7 makes the *improvement loop itself* better, by feeding TraceMind's own memory back into every layer of the stack. This is what "self-improving memory OS" has to mean in the year of recursive self-improvement — not a fine-tune, but a **closed loop over four improvement targets**, driven by real user behavior, running entirely on-device.

**Four improvement targets (L0 → L3):**

| Level | Target                                            | Cadence     | Owner (crate)              |
|-------|---------------------------------------------------|-------------|----------------------------|
| L0    | Bandit weights (LinUCB posteriors)                | per-query   | `tm-controller`            |
| L1    | Retrieval-policy prompts + verb weight-vectors    | nightly     | `tm-gepa` (Pillar 1)       |
| L2    | Space configuration (which spaces, what weights, promote/demote thresholds) | weekly | `tm-vector`, `tm-episodic` |
| L3    | The Curator itself — *which reflections become policies* — trained on TraceMind's own history | monthly | `tm-reflect` + `tm-gepa`   |

**Feedback signal fabric (input to all four levels):**

- **Explicit** — accept / reject on brief cards, thumbs on retrievals, edits to committed outcomes.
- **Implicit** — memory-reuse rate (was this retrieval cited?), dwell / follow-up query as miss-signal, silence after a proposal.
- **Behavioral** — which verbs the user actually invokes, when, from which host, in which session. Verb affinity is per-user and shifts over time.

Every signal is ingested as a first-class memory with source provenance. Signals never leave the device.

**User-behavior model:**

Runs *inside* TraceMind's own graph — the user's behavior model is itself a memory. Captures:
- **Verb affinity** — this user leans on `contradict` twice as often as median; weight that verb's Pareto axis higher for them.
- **Temporal patterns** — morning-brief user vs late-night-ideation user; different retrieval budgets, different promotion thresholds.
- **Host patterns** — Claude Code sessions want code context; Goose sessions want research context; scope Pillar 2 space weights accordingly.

**Recursive property:**

The Curator (L3) reads its **own** past mutations from TraceMind's memory. Reflections become memories → memories inform the next reflection → the Curator selects which reflection-patterns become the *next* Curator's prior. **TraceMind uses TraceMind to improve TraceMind.** The recursion is bounded — no self-modifying code, only self-modifying prompts, weights, and space-configs.

The unit of mutation is a `PolicyMutation`:
```
PolicyMutation {
  id: Uuid,
  parent_id: Uuid,              // which Pareto candidate was mutated
  mutation_kind: PromptParaphrase | WeightPerturb | SpaceAdd | SpaceRemove,
  delta_scores: [f32; 4],       // Δ on {F1, latency, contradiction_rate, multi_hop}
  accepted: bool,               // passed verifier gate?
  evidence: Vec<Uuid>,          // trace_ids that motivated this mutation
  generated_at: DateTime<Utc>,
}
```
The Curator receives: last 50 `PolicyMutation` records (sorted by `accepted` + `delta_scores`), rollback events as negative signals, and current Pareto archive. It outputs `PriorUpdate { amplify: Vec<MutationKind>, suppress: Vec<MutationKind> }` which shapes the next mutation batch's sampling distribution.

**Safety rails (non-negotiable):**

- Every policy change writes a **provenance edge** in the graph — `PolicyMutation → { motivation: reflection_id, evidence: [trace_ids], parent: prev_policy_id }`.
- User can view every mutation in the Tauri Brief and **roll back** any policy to any prior version. Rollback itself is a signal — repeated rollback of the same mutation family disables that mutation class.
- The anchor-eval verifier gate (Q4.3) prevents drift into local optima. If an anchor-eval regresses > 2 F1, the mutation is rejected and logged as a **negative example** for the next Curator round.
- Governance treats reflections as untrusted (they may echo prompt-injection from captured content).

## 6. Q3 Roadmap (Jul–Sep 2026)

Items are ordered by dependency: signal fabric first (everything in Q4 reads it), routing gate second (gates retrieval before arm selection), wedge continuity third (event graph is the storage foundation), then infrastructure, then the two high-risk items (BGE-M3 + ComposedIndex) placed where BGE-M3 has time to stabilise before ComposedIndex is built on top of it.

| # | Item | Crates | Est. LOC |
|---|------|--------|---------:|
| Q3.1 | **Feedback signal fabric** — explicit + implicit + behavioral schema, `memory_feedback` MCP verb, ingestion as first-class memories with provenance (Pillar 7 groundwork; **dependency root for all Q4 items**) | `tm-mcp`, `tm-ingest`, `tm-graph` | ~700 |
| Q3.2 | **Memory-routing gate** in `QueryPlanner` — binary `should_retrieve: bool` upstream of LinUCB, trained on Q3.1 feedback scores; NOT a bandit arm | `tm-controller`, `tm-retrieval` | ~500 |
| Q3.3 | EVG-1/2 + CTX-EVG completion (event graph freq-floor edges, wedge foundation) | `tm-graph`, `tm-episodic` | ~700 |
| Q3.4 | Temporal KG (`valid_from`/`valid_to`) + contradiction-rate metric definition (temporal-KG conflict detection, not string-diff) | `tm-graph`, `tm-temporal` | ~900 |
| Q3.5 | `session_id` / `host_id` scoping across MCP surface | `tm-graph`, `tm-mcp` | ~400 |
| Q3.6 | Hierarchical tiers (hot/warm/cold + promote/demote) in `tm-episodic` | `tm-episodic` | ~800 |
| Q3.7 | LongMemEval + BEAM harnesses (publish baseline before R5 window closes) | `tm-bench-locomo`, new `tm-bench-longmem` | ~1200 |
| Q3.8 | Reflexion post-mortem log + **GEPA feasibility spike** — verbal session post-mortem in `tm-reflect`; spike applies one GEPA mutation to the existing LinUCB weight vector and replays against `traces.jsonl` to validate Δ F1 > 0; result gates Q4.1 | `tm-reflect`, `tm-controller` | ~700 |
| Q3.9 | **BGE-M3 dense path only** (~768d) — fastembed ONNX integration via candle backend (BGE-M3 not yet in fastembed's official model list; requires direct ONNX loading), vector store schema migration (384→768d), latency validation ≤60ms p50. **Sparse and multi-vector outputs deferred to Q4.9.** | `tm-vector` | ~1800 |
| Q3.10 | `Space` trait + `ComposedIndex` MVP + query-time weights on MCP (depends on Q3.9 dense path stable) | `tm-vector`, `tm-mcp` | ~1000 |

**Q3 exit gate (four conjuncts, all required):**
1. `ComposedIndex` serves `memory_query` end-to-end with three spaces (text, recency, confidence) and per-verb weights.
2. LongMemEval baseline published.
3. Feedback signal fabric ingests all three signal classes; every retrieval carries a `feedback_hook_id`.
4. **GEPA spike result documented**: at least one synthetic mutation shows Δ F1 > 0 on `traces.jsonl` replay. If (4) fails, Q4.1-Q4.3 are blocked and the GEPA timeline moves to 2027 H1.

## 7. Q4 Roadmap (Oct–Dec 2026)

| # | Item | Crates | Est. LOC |
|---|------|--------|---------:|
| Q4.1 | GEPA reflective mutation loop over policy prompts | new `tm-gepa` | ~1500 |
| Q4.2 | Pareto archive on {F1, latency, contradiction, multi-hop} | `tm-gepa`, `tm-controller` | ~600 |
| Q4.3 | Anchor-eval verifier gate for policy updates | `tm-gepa`, `tm-bench-locomo` | ~500 |
| Q4.4 | Nightly self-improvement scheduler (on-device) | `tm-controller`, `tm-cli` | ~400 |
| Q4.5 | Memory-as-tools MCP verbs (pin/forget/promote/contradict/reflect) | `tm-mcp` | ~700 |
| Q4.6 | Working-memory pins first-class in ComposedIndex | `tm-vector`, `tm-reflect` | ~400 |
| Q4.7 | Composition-layer graph algebra verbs (∪ ∩ \\ filter bridge) | `tm-graph`, `tm-mcp` | ~900 |
| Q4.8 | Head-to-head bench harness vs Mem0/Zep/Letta/Engram | `tm-bench-memory` | ~800 |
| Q4.9 | Matryoshka slicing → hierarchical retrieval free-tier | `tm-vector` | ~300 |
| Q4.10 | Contradiction-rate as first-class Pareto axis | `tm-graph`, `tm-gepa` | ~400 |
| Q4.11 | ACE Curator (which reflections survive → policy library) | `tm-reflect`, `tm-gepa` | ~600 |
| Q4.12 | User-behavior model (verb affinity + temporal + host patterns) as first-class memory, feeds L2 space weights (Pillar 7). **Note**: `tm-world-model` currently runs logistic regression for `f_outcome` only; this item extends it with new feature dimensions (`verb_used`, `time_of_day`, `host_id`) and a new prediction target (`verb_affinity_score`). Extension, not rewrite. | `tm-world-model`, `tm-vector` | ~900 |
| Q4.13 | **Recursive meta-loop** — Curator reads its own past mutations + user-behavior model to shape next mutation batch; verifier gates; provenance edges written to graph (Pillar 7 headline). Concrete `PolicyMutation` schema: `{ id: Uuid, parent_id: Uuid, mutation_kind: PromptParaphrase\|WeightPerturb\|SpaceAdd\|SpaceRemove, delta_scores: [f32; 4], accepted: bool, evidence: Vec<Uuid>, generated_at }`. Curator prompt receives: last 50 `PolicyMutation` records ordered by `accepted` + `delta_scores`, rollback events, and current Pareto archive. Output: `PriorUpdate { amplify: Vec<MutationKind>, suppress: Vec<MutationKind> }`. | `tm-gepa`, `tm-reflect`, `tm-graph` | ~1200 |
| Q4.14 | Policy provenance + rollback UI in Tauri Brief; rollback events feed back as negative signals (Pillar 7 safety) | `tm-tauri`, `tm-graph`, `tm-controller` | ~700 |
| Q4.15 | Q4 exit-gate benchmark run + blog post + repo release | docs, CI | ~200 |

**Q4 exit gate prerequisite**: ≥3 active design partners with ≥15 real sessions each by 2026-10-01, OR synthetic bootstrap from `traces.jsonl` replay producing statistically stable F1 variance (σ < 0.5 F1 across 5 replay runs). If neither is met by 2026-10-01, Q4.1-Q4.3 ship as infrastructure only; the two-conjunct gate below is deferred to 2027 Q1.

**Q4 exit gate (falsifiable, two conjuncts — requires prerequisite above):**
1. **Δ F1 ≥ +3** on LongMemEval over the last 30 sessions of any user's real feedback (or 30-run synthetic replay if prerequisite was met via bootstrap), **with no code deploy in between**.
2. **Loop efficiency improves**: fewer GEPA rollouts per +1 F1 in the last 10 sessions than in the first 10 (the improvement loop itself is getting better — Pillar 7 signal).

If (1) fails, Pillar 1 loop doesn't close and fine-tuning gets revisited in 2027. If (2) fails, Pillar 7 recursion is decorative rather than load-bearing and we cut the L3 Curator.

## 8. Success Metrics

**Technical metrics** (infrastructure validity):

| Metric | Baseline (Jul 2026) | Q3 exit | Q4 exit |
|--------|---------------------|--------:|--------:|
| LoCoMo F1 | 49.27 | ≥ 49 | ≥ 52 |
| LongMemEval multi-hop | — (not measured) | baseline published | +3 vs baseline |
| Contradiction-rate | — | metric defined + published | < half of Mem0 |
| Retrieval p50 latency | ~40ms (BGE-small) | ≤ 60ms (BGE-M3 dense, 3 spaces) | ≤ 60ms |
| No-code-deploy Δ F1 over 30 sessions | 0 | GEPA spike Δ F1 > 0 (synthetic) | **≥ +3** |
| GEPA rollouts per +1 F1 (loop efficiency) | — | spike establishes baseline | **strictly decreasing** across sprint |
| User-verb-affinity model coverage | — | signals ingested | ≥ 4 verbs personalized per active user |

**User-visible metrics** (product validity — the seed story lives here):

| Metric | Baseline (Jul 2026) | Q3 exit | Q4 exit |
|--------|---------------------|--------:|--------:|
| Active design partners (≥15 sessions each) | 0 | ≥ 2 | ≥ 3 |
| Design partner W2 retention (≥3 active days wk 2) | 0% | ≥ 40% (gate) | ≥ 60% |
| "Brief card felt relevant" rate (DP survey, 1-5 scale) | — | baseline | ≥ 4.0 |
| Contradiction false-positive repeat rate after rollback | — | measured | < 10% |

## 9. Risk Table

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|-----------:|-------:|------------|
| R1 | GEPA loop doesn't converge on real user data | Med | High | Anchor-eval gate; fall back to fixed prompt if drift detected |
| R2 | BGE-M3 latency > 60ms p50 on mid-tier laptop | Med | Med | Matryoshka slicing (Q4.9); quantize to Q4; sparse-first pre-filter |
| R12 | fastembed doesn't ship BGE-M3 natively; direct ONNX loading required | High | Med | Q3.9 uses candle backend or raw ONNX runtime; fallback = BGE-small stays active until dense path proven |
| R3 | ComposedIndex becomes over-configurable | Med | Med | Fix set of 6 spaces + 4 verb weight-vectors; no user knobs |
| R4 | Contradiction-rate metric noisy | High | Med | Use temporal-KG conflict detection, not string diff |
| R5 | Engram or Mem0 ships local mode before us | Low | High | Ship Q3.10 ComposedIndex early; publish LongMemEval baseline (Q3.7) by Aug |
| R6 | GEPA + BGE-M3 exceeds 1.6GB footprint | Med | Med | Quantize; drop BGE-M3 multi-vector on mobile tier |
| R7 | Reflexion log becomes prompt-injection surface | Med | High | Governance gate reflections; treat as untrusted data |
| R8 | 30-session user-feedback baseline unattainable in H2 | High | Med | **Primary path**: synthetic feedback replay from `~/.tracemind/traces.jsonl` (confirmed viable at Q3.8 spike); real user data as secondary validation only. Q4 exit gate explicitly allows synthetic bootstrap. |
| R9 | Recursive Curator (L3) drifts into self-reinforcing local optimum | Med | High | Anchor-eval verifier + user rollback + rollback-events-as-negative-signals close the loop (Pillar 7 safety rails) |
| R10 | Feedback signal fabric picks up prompt-injection from captured content | Med | High | Governance gate on ingestion; reflections marked `trust: untrusted`; never expose reflection content to L3 without provenance filter |
| R11 | User-behavior model becomes a fingerprint / privacy leak if shared | Low | High | Model is local-only, never egresses; excluded from any future opt-in cloud tier by default |

## 10. Deprecations & Deferrals

- **QLoRA nightly Python sidecar** — obsoleted for retrieval by GEPA. Deferred to 2027; revisit only for generation-style personalization.
- **`mxbai-colbert` as separate reranker download** — will be replaced by BGE-M3 multi-vector output in Q4.9 once the dense path (Q3.9) is stable. Until then, mxbai-colbert remains active.
- **Scheduled `Consolidator` decay** — replaced by LinUCB `should_compact` arm (Pillar 3).
- **Standalone Reason navigation** — already removed per [[tracemind-reason-to-proactive]].
- **Cloud personalization tier** — off the roadmap; not a fit for the wedge.

## 11. Wedge Continuity

Everything in prior sprint memos still applies where not explicitly deprecated above. The concepts below are **canonical for H2** and unchanged:

- Commitment → Outcome primitive ([[tracemind-strategic-direction]])
- Event graph as first-class, entity-SVO as canonicalization sidecar ([[tracemind-event-graph-pivot]])
- Composition layer as graph algebra ([[tracemind-composition-layer]])
- EVG → ONT → LGM sprint ordering ([[tracemind-evg-ont-lgm-order]])
- Ontology as internal infra, no Foundry UI ([[feedback_no_foundry_ui]])
- Context segmentation, negative feedback, decoupled by default ([[tracemind-context-segmentation]])
- Host-agnostic MCP (superseded on hosts count, retained on architecture)

## 12. Charter Delta vs `PROJECT_2026.md`

| Area | PROJECT_2026 | H2 Charter |
|------|--------------|------------|
| Personalization mechanism | QLoRA Python sidecar | GEPA reflective mutation (Q4, gated on Q3.8 spike) |
| Retrieval architecture | BGE-small + mxbai-colbert reranker | BGE-M3 dense (Q3) + ComposedIndex (Q3) + BGE-M3 multi-vector (Q4) |
| Memory routing | None | `QueryPlanner` gate (`should_retrieve`) upstream of LinUCB — not a bandit arm |
| Consolidation | Scheduled `Consolidator` | LinUCB `should_compact` arm (Pillar 3) |
| Self-improvement | None explicit | Pillar 1 (retrieval-policy) + Pillar 7 (recursive across L0/L1/L2/L3) |
| Benchmarks | LoCoMo only | LoCoMo + LongMemEval + BEAM + contradiction-rate + loop-efficiency |
| MCP verbs | store / query / trace / reason / analogies / consolidate | + pin / forget / promote / contradict / reflect / feedback |
| User-behavior modeling | None | Extension of `tm-world-model` with verb/host/time features; drives L2 space weights |
| Policy provenance & rollback | None | `PolicyMutation` records edged in graph; user rollback in Tauri (Pillar 7 safety) |
| User-visible story | None explicit | Three observable behaviors in Pillar 6 — verb rhythm, composition suggestions, non-repeating contradictions |

## 13. Open Questions

- Does Reflexion post-mortem run per-turn, per-session, or per-day? (Q3.8 discovery)
- ~~Should the no-retrieve arm be a LinUCB arm or a separate router upstream?~~ **Resolved: separate QueryPlanner gate, not a bandit arm (Q3.2).**
- Contradiction-rate metric: soft (embedding cosine) or hard (temporal KG conflict)? **Strong default: temporal-KG conflict (Q3.4); define precisely before building the metric.**
- BGE-M3 ONNX loading: candle backend vs. raw `ort` runtime vs. wait for fastembed support? (Q3.9 decision; affects build complexity)
- Matryoshka slicing granularity for BGE-M3 768d: 128 / 256 / 384 / 512? (Q4.9 tuning)
- GEPA mutation granularity: per-policy-prompt only, or also per-arm-weight-vector? (Q3.8 spike will answer this)
- Curator recursion cadence: nightly, weekly, or event-driven (N rollbacks / M reflections)? (Q4.13 discovery)
- User-behavior model bootstrap: cold-start prior from LoCoMo synthetic feedback, or wait for real user history? (Q4.12 decision)
- Loop-efficiency metric: rollouts-per-+1-F1 or wall-clock-per-+1-F1? (Q4.15 exit-gate definition)

## 14. Signing Statement

The wedge from every prior charter still holds. H2 2026 is about **closing the local self-improvement loop credibly and cheaply** — GEPA + ComposedIndex + hierarchical tiers + memory-as-tools verbs.

The build is sequenced to derisk the two biggest bets early: feedback signal fabric goes first (Q3.1) so Q4 has real signals to learn from; GEPA gets a spike in Q3 before a full `tm-gepa` crate is committed. If either gate fails, the quarter's remaining work is not wasted — ComposedIndex, hierarchical tiers, and the benchmark suite are standalone improvements.

If the Q4 exit gate (Δ F1 ≥ +3, no code deploy) fires, TraceMind is architecturally what no cloud personalization vendor can be. If it doesn't, we revisit fine-tuning in 2027 — but the wedge (local, user-owned, composed) still holds either way. The user-visible story (verb-rhythm briefs, cross-session composition, non-repeating contradictions) remains true regardless of whether GEPA closes.

---

*Charter effective 2026-07-07. Revised 2026-07-21: Q3 dependency ordering, memory-routing gate architecture, BGE-M3 phased rollout, GEPA feasibility gate, user-visible narrative, PolicyMutation schema, Q4 exit gate prerequisites. Owner: Aaditya Srivathsan. Prior canonical: `docs/PROJECT_2026.md` (retained for lineage).*
