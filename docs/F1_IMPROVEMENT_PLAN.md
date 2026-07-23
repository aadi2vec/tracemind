# F1 Improvement Plan — 2026-07-22

**Current state:**
- LoCoMo F1 = **50.32** on fresh conversations (the overfit 75.49 is retired; see `docs/MVP-STATUS-2026-07.md`)
- Q3 exit gate target: ≥ 49 (currently met)
- Q4 exit gate target: ≥ 52 + Δ F1 ≥ +3 over 30 sessions no-code-deploy
- Competitive minimum to publish head-to-head vs Mem0/Zep/Letta/Engram: **≥ 60**

This document is an **ordered, F1-focused** todo list. It intentionally excludes charter items that don't move F1 (Tauri rollback UI, provenance edges, blog post, etc.) even though they matter for other reasons.

Ordering discipline: **expected F1 lift ÷ estimated LOC**, with dependencies respected.

---

## Tier 1 — do first (high leverage, low cost, no unblocking needed except within tier)

### F1.1 — BGE-M3 dense wiring end-to-end (finish Q3.9)
- **Current:** scaffold only; `BgeM3DenseModel` returns a descriptive error, runtime falls back to hash embedder (which is why real F1 vs bench F1 diverged for so long)
- **Expected lift:** **+3–5 F1** on retrieval-dependent categories (single_hop, name_resolution, factual_recall); ~+2 overall
- **Cost:** ~1500 LOC — `ort` runtime + `tokenizers` + vector-store schema migration 384→768d + p50 ≤60ms validation on M-series
- **Blocking:** nothing
- **Rationale:** every downstream item in Tiers 2/3 assumes real embeddings. Nothing else here is worth measuring until this ships.

### F1.2 — Benchmark hygiene (never tune on `locomo-mini`)
- **Current:** eval-invalid memory says we tuned on the same set we scored on
- **Expected lift:** **0 F1** — but makes every future number trustworthy; prevents another 75→50 correction
- **Cost:** ~200 LOC — split `locomo-mini` into tune/test with fixed seed; run LongMemEval + BEAM baseline (harness already shipped Q3.7)
- **Blocking:** nothing
- **Rationale:** if we skip this, F1.3–F1.11 measurements are noise.

### F1.3 — Wire GEPA output back into runtime (close the loop)
- **Current:** `tm-gepa` crate exists with 38 unit tests but is unwired — no path from `GepaLoop.run_round()` output to LinUCB / ComposedIndex weights at runtime; verifier can't fail because nothing depends on it
- **Expected lift:** +0 immediately, **+1–3 over 30 sessions** as the loop starts closing; unlocks Q4 exit gate mathematically
- **Cost:** ~400 LOC — `LinUcbBandit::apply_gepa_weights()` + `ComposedIndex::apply_verb_weights()` + wire nightly scheduler to call both after verifier passes
- **Blocking:** F1.1 (verifier gate is meaningless with hash embeddings)

### F1.4 — Feedback signal → LinUCB reward wiring
- **Current:** `memory_feedback` verb ingests signals into `feedback_signals` table (Q3.1 shipped), but do those signals actually update LinUCB posteriors, or do we still only use the synthetic relevance score?
- **Expected lift:** **+1–2 F1** as bandit learns from real user rewards, not synthetic
- **Cost:** ~200 LOC — `FeedbackSignal → reward` mapping (accept=1.0, reject=0.0, reuse=0.8, dwell>Xs=0.6, follow-up-miss=0.2) + `LinUcbBandit::register_reward` from the feedback ingestion path
- **Blocking:** nothing (Q3.1 shipped)

### F1.5 — Grammar-constrained decoding for Tier-1 extractor
- **Current:** Tier 1 (`local-llm` feature) is scaffolded per CLAUDE.md; span extractors are regex (per `tm-bench-locomo::extract`)
- **Expected lift:** **+2–3 F1** on structured answer categories (temporal, name_resolution, quantity)
- **Cost:** ~600 LOC — `llama-cpp-2` grammar hookup + GBNF schemas per answer type (date, person, place, choice, value, quantity, duration)
- **Blocking:** llama-cpp-2 actually loading a Qwen 2.5 1.5B Q4 GGUF (partially there in scaffolding)

**Tier 1 sub-total:** ~2900 LOC, **~+6–10 F1 direct + loop-closing enablement** → puts us at **56–60 baseline**.

---

## Tier 2 — do next (medium leverage, medium cost, some depend on Tier 1)

### F1.6 — Q3.3 EVG-1/2 event graph completion (unskip the merge gap)
- **Current:** skipped by the H2 merge (`docs/CHARTER-H2-2026.md` Q3.3 listed, not implemented)
- **Expected lift:** **+2–4 F1** on multi_hop and temporal — gives structural queries a real descent target instead of flat top-K
- **Cost:** ~700 LOC — freq-floor edges on the event graph + `session_scope` join in retrieval planner
- **Blocking:** nothing
- **Rationale:** the RLM discussion (see `docs/AIE_2026_BACKLOG.md`) is arguing this is more load-bearing than we treated it. It's also the substrate F1.9 and F1.11 need to descend through.

### F1.7 — Multi-query fusion on ComposedIndex (backlog §3.8 → promote)
- **Current:** single-query only; per-verb weights ship but no paraphrase branching
- **Expected lift:** **+1–2 F1** on paraphrased evals, especially LongMemEval `multi_hop`
- **Cost:** ~300 LOC — Tier-0 templated paraphrases (no LLM), RRF fusion, capped at N=3
- **Blocking:** F1.1 (BGE-M3 dense — otherwise you're fusing noise)

### F1.8 — Salience layer as 5th ComposedIndex space (backlog §3.6 → promote)
- **Current:** no `SalienceSpace`; only Recency/Confidence/Text/EntityType/HostSession
- **Expected lift:** **+1–2 F1** by demoting stale/unused memories from top-K
- **Cost:** ~400 LOC — `SalienceScore = f(recency, reuse_rate, verb_affinity_hit_rate)` derived from Q3.1 signal fabric
- **Blocking:** F1.4 (needs real feedback signals feeding reuse_rate)

### F1.9 — BridgeRAG multi-hop bridge stage (backlog §3.3 → promote if trigger fires)
- **Current:** implicit bridging via KG-R1 hop expansion; no explicit bridge scoring
- **Expected lift:** **+2–4 F1** on `multi_hop` specifically
- **Cost:** ~600 LOC — new `PlanAction::Bridge` in QueryPlanner + `BridgeSpace` scorer
- **Blocking:** F1.6 (event graph as bridge target), gate on F1.2 baseline showing multi_hop lags single_hop by > 8 pts

**Tier 2 sub-total:** ~2000 LOC, **~+6–12 F1** → puts us at **62–72 range**.

---

## Tier 3 — bigger bets (higher cost, higher risk, longer horizon)

### F1.10 — Template-parametric GEPA (from RLM discussion)
- **Current:** GEPA mutates policy prompt strings; no template inventory
- **Expected lift:** **+2–5 F1** as templates are debuggable and reliable at 1.5–7B scale; also better generalization than free-form mutation
- **Cost:** ~800 LOC — ~20 hand-written decomposition/extraction templates + selection prompt + `MutationKind::TemplateSwap` and `TemplatePatch` variants for GEPA
- **Blocking:** F1.3 (GEPA wired), F1.5 (grammar-constrained decoding so template outputs are enforceable)

### F1.11 — RLM depth-2 recursion prototype (new architectural bet)
- **Current:** flat `QueryPlanner → LinUCB arm → ComposedIndex + graph expansion`
- **Expected lift:** **+3–8 F1** on structural queries IF hypothesis holds; single biggest lever we have; also single biggest risk of adding cost with no lift
- **Cost:** ~1500 LOC — root decomposer, sub-call composer over EVG + typed-AST, per-node verifier, depth-2/wide-fanout budget governor
- **Blocking:** F1.6 (event graph), F1.10 (templates as sub-call interfaces)
- **Gate before starting:** F1.2 baseline + F1.7 shipped → measure `structural_query_gap`. If flat retrieval already clears 65 F1 on multi_hop, RLM's cost isn't justified. Run **as an experiment, not a commit**, per the discussion's own advice.

**Tier 3 sub-total:** ~2300 LOC, **~+5–13 F1** with real risk of 0 → best-case puts us at **67–85**, more realistically **62–70** with one of the two paying off.

---

## Do-not-do list (things that look F1-shaped but aren't for us)

- **Cross-user population learning** — off-wedge, positioning-locked to individual-only (see charter Positioning Commitment when added)
- **Population-based prompt search (HyperAgents)** — compute budget doesn't allow; F1 lift would require cloud
- **LoRA / QLoRA sidecar** — deferred; wrong bet for retrieval per GEPA replacement
- **Long-context transformer as fallback** — Tier-1.6GB budget doesn't allow; 2027
- **Transfer experiment from external strategy discussion** — moot under individual-only positioning
- **Larger local model (7–8B) as default Tier 1** — breaks the installed base; keep as opt-in "beast mode" only
- **New embedding model beyond BGE-M3** — churn cost > lift; BGE-M3 is state-of-art for local for at least 6 more months

---

## Suggested execution sequence

- **Sprint 1 (2–3 weeks):** F1.1 (BGE-M3 wiring) + F1.2 (bench hygiene) in parallel. Target: **trustworthy 55 F1 baseline**.
- **Sprint 2:** F1.3 (GEPA wired) + F1.4 (feedback → reward). Target: **loop closes; F1 starts moving without deploys**.
- **Sprint 3:** F1.5 (grammar-constrained) + F1.6 (EVG). Target: **structural query lift + extraction quality**.
- **Sprint 4:** F1.7 (multi-query) + F1.8 (salience). Target: **60+ baseline, ready for competitive head-to-head**.
- **Sprint 5 (research spike):** F1.9 (BridgeRAG) *if* the multi_hop gap warrants it, F1.10 (template GEPA) as a preview.
- **Sprint 6+:** F1.11 (RLM prototype) as an experiment, hard-gated on structural gap in F1.2 measurements.

---

## Success signal per sprint

Every sprint ends with a `tm-bench-locomo` + `tm-bench-longmem` + BEAM run posted to `docs/blog/` or a status memo. **No sprint completes without a benchmark run on the fresh test split.** This is the discipline that failed pre-2026-07 and gave us the 75→50 correction.
