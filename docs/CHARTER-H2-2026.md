# TraceMind Charter — H2 2026

**Effective:** July 7, 2026
**Supersedes:** `PROJECT_2026.md` for H2 planning (Jul 1 – Dec 31, 2026)
**Status:** Pivot. Wedge preserved, mechanism replaced.

---

## 0. TL;DR

The 2026 memory market split into **cloud personalization APIs** (Mem0, Zep, Engram, Letta hosted) and **framework-embedded memory blocks** (LangMem, self-editing state). Both surrender the two properties H2 rewards: **user-owned graph** and **retrieval that improves without a training pipeline**.

TraceMind's H2 bet: a **local-only, self-improving memory substrate** — MCP server underneath any host agent — that gets measurably better every week from user feedback alone. No fine-tune. No cloud. No lock-in.

The pivot: swap the earlier "scheduled decay + heuristic consolidator" mechanism for **GEPA-driven prompt evolution** over a **composed multi-space retrieval index**. The wedge (Commitment→Outcome, event graph, composition across hosts) is unchanged.

---

## 1. What Changed Since `PROJECT_2026.md`

Five signals since April 2026 justify a pivot in mechanism (not thesis):

1. **AIE World's Fair 2026 crystallized "self-improvement" as a category.** Anthropic / DeepMind / Amazon AGI keynoted recursive self-improvement loops as production practice. Memory & Continual Learning became a first-class track.
2. **ACE, GEPA, Reflexion converged.** A well-formed pattern (Generator → Reflector → Curator → Verifier → next-turn policy) with published +10.6% agent-benchmark lift, no fine-tune.
3. **GEPA obsoletes LoRA for our use case.** Genetic-Pareto Prompt Evolution beats GRPO on multi-hop QA with ~35× fewer rollouts, no weight updates, fully explainable prompt diffs. Removes the need for a Python training sidecar.
4. **NirDiamant catalog (30 techniques) confirmed the substrate gaps.** Hierarchical tiers, memory routing, memory-as-tools verbs, working-memory pins — all absent from TraceMind, all foundational.
5. **Retrieval field moved past dense-only.** BGE-M3 (dense + sparse + multi-vector in one model), Matryoshka embeddings (free hierarchical retrieval), BridgeRAG (multi-hop bridges), Superlinked's Spaces pattern (query-time weight vectors over composed indexes). Our flat BGE-small + separate rank fusion is 18 months behind.

Two competitive updates:

- **Engram Labs** is a cloud personalization API pointed at consumer memory. Same market, opposite architecture. We win by being local + reversible + host-agnostic; we cannot win on scale.
- **Mem0 / Zep / Letta** now share LoCoMo + LongMemEval as the field benchmark. Contradiction rate is the axis they can't win on architecturally (no temporal edges). This is our legibility beachhead.

---

## 2. Thesis (updated)

> A memory system for AI agents is only defensible if (a) the graph is owned by the user, (b) the retrieval policy compounds on the user's own feedback, and (c) it plugs under any host, not into one framework.
>
> TraceMind is the substrate that satisfies all three, shipped as a Rust MCP server that never phones home and gets strictly better every 30 sessions without a training pipeline, without an adapter, without a cloud call.

---

## 3. Positioning

| Player          | Substrate           | Feedback loop        | Where they lose to us              |
|-----------------|---------------------|----------------------|-------------------------------------|
| Mem0            | Cloud vec + graph   | Manual eval          | Data leaves device; no contradictions |
| Zep / Graphiti  | Cloud temporal KG   | Manual eval          | Cloud; single-vendor                 |
| Engram          | Cloud personalization API | Opaque         | Not user-owned; opaque adapter       |
| Letta hosted    | Cloud MemGPT tiers  | Model-driven edits   | Cloud; framework-locked              |
| LangMem         | In-process state    | None                 | Not persistent across hosts          |
| **TraceMind**   | **Local SQLite + composed vector + temporal KG** | **GEPA + LinUCB on user feedback** | *H2 job: prove the loop closes* |

---

## 4. Non-Goals for H2

Explicitly deferred:

- **No cloud tier.** Optional encrypted-cloud for low-end devices stays speculative through 2026.
- **No fine-tuning pipeline.** GEPA replaces the earlier QLoRA sidecar plan. See §10 for LoRA deferral rationale.
- **No standalone chat UI.** Tauri Brief consumes MCP; not a rival to Claude Desktop.
- **No enterprise / team features.** Multi-agent shared memory is single-user, multi-host only.
- **No ontology UI.** Internal infrastructure only. Settings sub-panel + accept/reject prompts.
- **No new host surface beyond Claude Code + Goose in H2.** Cursor/Zed are 2027 targets.
- **No image or audio modalities.** Text-first through 2026.

---

## 5. Pillars

### Pillar 1 — GEPA-driven self-improving policy *(the differentiator)*

By 12/31, TraceMind's retrieval quality on LoCoMo + LongMemEval + BEAM must be **strictly monotonic over the last 30 sessions of any user's real feedback**, with no code deploy in between.

Mechanism:
```
LinUCB reward → Reflexion post-mortem → GEPA reflective mutation
              → Pareto archive → Verifier gate → next-query policy
```

GEPA evolves prompts + weight vectors across:
- `QueryPlanner` classification prompt
- `Reflector` post-mortem template
- `Verifier` critic prompt
- `Consolidator` compaction instruction
- Per-arm retrieval instructions
- **Per-verb default weight vectors** (see Pillar 2)

Pareto axes: `{F1, latency_ms, contradiction_rate, LongMemEval_multi_hop}`. A candidate promotes to the archive if it dominates on any axis.

**No competitor can match this claim locally.** This is the paper-quality moat.

### Pillar 2 — Composed multi-space retrieval *(second differentiator)*

Replace flat BGE-small + separate signal fusion with a **`ComposedIndex`** (Superlinked-pattern, Rust-native) combining:

- Text (BGE-M3: dense + learned-sparse + multi-vector, one model, one forward pass)
- Recency (exponential decay space)
- Confidence (from `tm-governance`)
- Entity type (categorical)
- Signal source (categorical)
- Host / session scope (categorical, for multi-tenant safety)

**Query-time weight vectors** expose the same index as many verbs (Recall, Contradict, Anticipate, Commit). GEPA (Pillar 1) evolves default weight vectors per verb.

Matryoshka slicing gives hierarchical-tier retrieval for free: 128d for cold, 384d for warm, full 768d for hot.

### Pillar 3 — Hierarchical substrate + memory-as-tools verbs

Flat SQLite blob becomes hot/warm/cold tiers with promotion/demotion. `tm-mcp` gains verbs:

- `memory_pin` — host pins salient items into hot tier
- `memory_forget` — host requests forget with reason
- `memory_promote` — cold → warm on demand
- `memory_contradict` — host asserts a fact supersedes another (writes valid_from/valid_to)
- `memory_reflect` — host triggers Reflector on a trajectory

Makes `[[tracemind_composition_layer]]` literal: hosts have verbs, not just deposit-and-retrieve.

### Pillar 4 — Legibility as a benchmark story

Ship **TraceMind vs Mem0 vs Zep vs Letta** head-to-head, public, reproducible in-repo:

- LoCoMo (already shipping — 49.27 F1 baseline)
- LongMemEval (add Q3)
- BEAM (add Q3)
- Contradiction rate on LongMemEval-temporal (add Q3, our differentiating axis)
- Latency p50 / p95

Reinforced by **markdown export** and Karpathy/Obsidian PKM zero-manual capture — the user-owned story stays legible.

### Pillar 5 — Two-surface ship discipline

MCP server is P0 (Claude Code + Goose). Tauri Brief is P1. CLI is maintenance. No new surfaces. Ambient capture and demo fixture from the demo punch list shippable by end Q3.

### Pillar 6 — Wedge continuity

Non-negotiable, carried from `PROJECT_2026.md`:

- Commitment → Outcome bond loop (CTX-EVG-C shipped Q2)
- Event graph over entity-SVO (canonical direction)
- EVG → ONT → LGM sprint order preserved
- No Foundry-style ontology UI

---

## 6. Q3 2026 (Jul – Sep) — Foundation

Ordered by dependency. Each item shippable in ≤ 2 weeks.

| # | Deliverable | Crate | Signal |
|---|-------------|-------|--------|
| Q3.1 | Memory Routing classifier + no-retrieve LinUCB arm | `tm-ingest`, `tm-controller` | 30–50% p50 latency drop on follow-ups |
| Q3.2 | Hierarchical tiers (hot/warm/cold) w/ promotion/demotion | `tm-episodic` | Substrate for Q4 loop items |
| Q3.3 | LongMemEval + BEAM harnesses | `tm-bench-locomo` | Delta-verifiable baseline |
| Q3.4 | Temporal KG with `valid_from`/`valid_to` + contradiction-rate metric | `tm-graph`, `tm-temporal` | Closes update-semantics gap vs Zep |
| Q3.5 | `session_id` / `host_id` scoping across trace + store | `tm-graph`, `tm-mcp` | Multi-host safe before Goose onboard |
| Q3.6 | Reflexion post-mortem log (`reflections.jsonl`) | `tm-reason` | Prereq for GEPA (Q4.2) |
| Q3.7 | EVG-1/2 + CTX-EVG completion | `tm-graph` | Wedge held |
| Q3.8 | BGE-M3 swap-in (dense + sparse + multi-vector in one model) | `tm-vector` | Retires mxbai-colbert as separate model |
| Q3.9 | `Space` trait + `ComposedIndex` MVP (text + recency) + query-time `weights` on MCP | `tm-vector`, `tm-mcp` | Superlinked pattern in Rust |

**Q3 exit gate:**
- LoCoMo F1 ≥ 52 (from 49.27)
- LongMemEval baseline recorded (all four splits)
- One head-to-head published (TraceMind vs Mem0 on LoCoMo + LongMemEval)
- Composed index in production for `memory_query`

---

## 7. Q4 2026 (Oct – Dec) — The Loop Closes

| # | Deliverable | Crate | Signal |
|---|-------------|-------|--------|
| Q4.1 | Memory-as-Tools verbs in `tm-mcp` (`pin`, `forget`, `promote`, `contradict`, `reflect`) | `tm-mcp` | Composition layer operational |
| Q4.2 | GEPA mutation operator + Pareto archive (`PromptArchive`) | `tm-controller`, `tm-reason` | Pillar 1 core |
| Q4.3 | Verifier (GVU) — retrieval + Reflector output | `tm-answer` | Judge-critic gate for GEPA promotion |
| Q4.4 | GEPA rollout to Reflector, Consolidator, per-arm reranker prompts, per-verb weight vectors | `tm-controller` | Full policy surface under GEPA |
| Q4.5 | Slipstream-style trajectory-grounded compaction validation | `tm-reason` | No more "compaction ate the important thing" |
| Q4.6 | Confidence, entity-type, signal-source Spaces added to `ComposedIndex` | `tm-vector` | Full Superlinked pattern |
| Q4.7 | BDTR bridge hypothesis + iterative-retrieve loop (LinUCB-gated) | `tm-retrieval` | Multi-hop wins |
| Q4.8 | Matryoshka slicing by tier at query time | `tm-vector` | Free 6× cold-tier speedup |
| Q4.9 | Reasoning-guided retrieval (Tier-1 thinking-trace tokens as query vectors) | `tm-answer`, `tm-retrieval` | Local-only differentiator |
| Q4.10 | Voyager-style skill promotion via GEPA-scored candidates | `tm-episodic` | `ProcedureStore` no longer a stub |
| Q4.11 | Anchor-eval set (100 held-out user-labeled trajectories) for GEPA scoring | `tm-bench-locomo` | Honest Pareto rankings |
| Q4.12 | ONT-2 → LGM-2 → ONT-3 per prior roadmap | `tm-graph` | Wedge held |

**Q4 exit gate — charter success:**

- Monotonic 30-session improvement demonstrated on ≥ 1 real user's feedback log
- Head-to-head: TraceMind ≥ parity with Mem0 on LongMemEval, strict win on contradiction-rate
- MCP verbs adopted by ≥ 1 external host beyond Claude Code (Goose or third-party)
- Public reproducible bench in-repo

---

## 8. Success Metrics

| Metric | Baseline (Q2) | Q3 exit | Q4 exit |
|--------|--------------|---------|---------|
| LoCoMo F1 | 49.27 | ≥ 52 | ≥ 55 |
| LongMemEval overall | — | baseline recorded | ≥ 60 |
| BEAM | — | baseline recorded | ≥ 50 |
| Contradiction rate (LongMemEval-temporal) | — | ≤ 5% | ≤ 2% |
| p50 hot retrieval | ~120ms | ≤ 80ms | ≤ 60ms |
| p50 cold retrieval | ~350ms | ≤ 250ms | ≤ 200ms |
| Self-improvement Δ F1 over last 30 sessions (no deploy) | — | — | **≥ +3** |
| Host integrations in production | 1 (Claude Code) | 2 (+ Goose) | 2+ |
| GitHub stars | ~40 | ≥ 250 | ≥ 500 |
| External `tm-bench-locomo` contributors | 0 | ≥ 1 | ≥ 2 |

**The +3 F1 monotonic-improvement number is the falsifiable Pillar 1 claim.** If it doesn't happen, the loop doesn't close, and we revisit fine-tuning in 2027.

---

## 9. Risks & Pre-Mortem

| # | Risk | Likelihood | Mitigation |
|---|------|-----------|------------|
| R1 | GEPA loop doesn't close — LinUCB reward too noisy to steer Pareto mutation | Medium | Q3.6 Reflexion log runs standalone before Q4.2 GEPA; if reflections alone don't lift F1 in Q3, reward signal is bad before we build the archive |
| R2 | Cloud incumbents ship local-first mode | High | Moat is the *loop*, not the substrate; ship Pillar 1 fast, publish benchmarks, ensure reproducibility |
| R3 | Claude Code changes MCP protocol | Medium | Goose as second surface is not optional; Q3.5 host_id scoping locks portability in schema |
| R4 | Founder bandwidth — 9 items Q3, 12 items Q4 | High | Q3.1 / Q3.6 are ~200 LOC each; Q4 items depend on Q3 substrate; explicit non-goals above; parallelization is limited by review capacity not code capacity |
| R5 | GEPA anchor set biases toward historical distribution — new query types don't get evolved | Medium | Q4.11 anchor set rotates monthly; log divergence between anchor-eval and live-feedback rewards |
| R6 | BGE-M3 heavier than BGE-small (~570MB vs ~110MB) | Low | Still under 1GB Tier-1 footprint budget; ships bundled or auto-download |
| R7 | Composed vector fragmentation (each Space needs re-tuning as data shifts) | Medium | GEPA evolves the weights; drift becomes signal, not bug |
| R8 | Engram or Mem0 releases a "local mode" mid-H2 | Medium | Reversibility + no-cloud + Pareto contradiction-rate story is architectural, not featural — they can't ship reversibility overnight |

---

## 10. Deprecations & Deferrals

**Removed from prior plans:**

- **QLoRA Python sidecar for nightly personalization** — obsoleted by GEPA. GEPA delivers compounding improvement without weight modification, keeping the "no fine-tune, no cloud" story intact. Revisit only if the Pillar 1 anchor-eval reveals GEPA cannot cover a specific capability class (candidate: personalized generation style, not retrieval).
- **Scheduled `Consolidator` decay** — replaced by compaction-as-decision under LinUCB `should_compact` arm.
- **`mxbai-colbert` as separate reranker model** — replaced by BGE-M3 multi-vector output. One model, one download.
- **Standalone Reason navigation** — already deleted in Q2; verbs surface as action cards in Brief + Dashboard. Kept out.

**Deferred to 2027:**

- QLoRA reranker sidecar (only if GEPA insufficient)
- Cursor / Zed host integrations
- Image / audio modalities
- Enterprise / team features
- Opt-in encrypted cloud tier

---

## 11. Wedge Continuity (unchanged from prior charters)

- **System of intents.** Commitment is the primitive; Outcome closes the bond loop.
- **Event graph over entity-SVO.** Frequency-floor edges, three graphs per consumer loop.
- **Composition layer.** Threads are first-class; graph algebra (`∪`, `∩`, `\`, `filter()`, `bridge()`) composes per-thread context.
- **Host-agnostic MCP first.** Ship into Claude Code + Goose; Tauri app is Brief + Dashboard.
- **Karpathy/Obsidian PKM zero-manual capture** with markdown export (`tracemind export` shipped 2026-05-12).
- **Deny-list as auto-correct** (`tm-graph::deny_list` scaffold shipped 2026-05-12).
- **Ontology as internal infrastructure**, not UI.
- **Layered graph reasoning:** Ontology / Episodic / Semantic / Salience / Knowledge layers.

---

## 12. Charter Delta Table — vs `PROJECT_2026.md`

| Concern | `PROJECT_2026.md` (Apr) | This charter (Jul) |
|---------|------------------------|--------------------|
| Self-improvement mechanism | Manual tuning + scheduled decay | **GEPA + LinUCB + Reflexion** |
| Fine-tuning story | QLoRA nightly (opt-in) | **Deferred; GEPA obsoletes for retrieval** |
| Retrieval substrate | BGE-small + graph + ColBERT (separate models, RRA fusion) | **BGE-M3 (unified) + ComposedIndex (multi-space) + Matryoshka tiers** |
| Consolidation | Scheduled decay + merge | **LinUCB-gated + Slipstream-validated** |
| Bench axes | LoCoMo F1 + EM | **LoCoMo + LongMemEval + BEAM + contradiction-rate + latency Pareto** |
| Memory verbs | `store` + `query` | **+ `pin`, `forget`, `promote`, `contradict`, `reflect`** |
| Host surface | Claude Code + Tauri | Claude Code + **Goose** + Tauri |
| Wedge | Commitment→Outcome, event graph, composition | *unchanged* |

---

## 13. Open Questions (to resolve in Q3 planning)

1. **Anchor-eval labeling policy.** Who labels the 100 held-out trajectories? Founder-only for Q3; open to power users in Q4?
2. **GEPA promotion cadence.** Every generation? Every N sessions? Nightly consolidation window?
3. **BGE-M3 storage cost.** Multi-vector output is ~16KB/doc — at what corpus size do we tier it into cold-only?
4. **Reasoning-guided retrieval budget.** Tier-1 thinking traces cost tokens; how do we gate against runaway iteration?
5. **Deny-list ↔ GEPA interaction.** If a user's deny-list flags an entity, should GEPA down-weight recency/text spaces that surface it, or is it purely a post-filter?

Answered by end of first Q3 sprint planning.

---

## 14. Signing Statement

We are not building Mem0 but local. We are building the substrate that makes cloud memory APIs obsolete for individual users. The loop closes locally, or it doesn't matter that we can't be sold.

— H2 2026 charter, effective 2026-07-07
