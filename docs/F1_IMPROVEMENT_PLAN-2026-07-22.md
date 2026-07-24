# TraceMind Improvement Plan (F1 + Holistic Review, merged)

**Created:** 2026-07-22 · **Updated:** 2026-07-23 (merged the holistic review in; status reconciled against shipped code)

This is the single improvement roadmap. It merges the F1-retrieval track (F1.x) and the holistic-review product/robustness track (H.x) into one ordered, status-marked list. Every item is marked ✅ done, 🟡 partial, or ⬜ todo, verified against the code — nothing is marked done that isn't in `main`.

**Current state (verified):**
- LoCoMo F1 = **50.32 on fresh conversations** (`locomo-train`, the tuning split), **70.49 on the held-out `locomo-mini`**. The overfit 75.49 is retired.
- Retrieval is genuinely load-bearing: BGE-small **70.49** vs hash **48.33** on held-out — a 22-pt spread where they used to tie byte-for-byte.
- MCP tool selection **80%** at p95 **51ms** (first ever measured, up from 55%).
- **604 tests green** across touched crates.
- Competitive minimum to publish head-to-head vs Mem0/Zep/Letta/Engram: **≥ 60 F1**.

**Disciplines that must hold:** never tune on `locomo-mini`; no sprint completes without a `tm-bench-locomo` run on the held-out split; never re-expand the advertised MCP surface past the core 6.

---

## Status at a glance

**Retrieval-F1 track**

| # | Item | Status |
|---|------|--------|
| F1.2 | Benchmark hygiene / train–test split | ✅ done |
| F1.3 | Wire GEPA output into runtime (executing verifier) | ✅ done |
| F1.4 | Feedback signal → bandit reward (evidence-gated) | ✅ done |
| F1.1 | BGE-M3 dense (768d) end-to-end | ⬜ todo (re-scoped) |
| F1.5 | Grammar-constrained Tier-1 extractor | ⬜ todo |
| F1.6 | Event graph (EVG-1/2) completion | ⬜ todo |
| F1.7 | Multi-query fusion on ComposedIndex | ⬜ todo |
| F1.8 | Salience layer as 5th space | ⬜ todo |
| F1.9 | BridgeRAG multi-hop bridge stage | ⬜ todo (gated) |
| F1.10 | Template-parametric GEPA | ⬜ todo |
| F1.11 | RLM depth-2 recursion prototype | ⬜ todo (experiment) |

**Holistic-review track (product / robustness / self-improvement)**

| # | Item | Status |
|---|------|--------|
| H.1 | Core-6 MCP surface (subtract from 51 tools) | ✅ done |
| H.2 | `tm-bench-mcp` — measure the MCP server | ✅ done |
| H.3 | Sharpen core-6 descriptions (55→80% selection) | ✅ done |
| H.4 | GEPA over tool descriptions | 🟡 built; held for real data |
| H.5 | Retraction beat — auto-detect + surface + instrument | ✅ done |
| H.6 | Temporal supersession (`valid_to` on reversed facts) | ✅ done |
| H.7 | Two-stage ANN — remove silent recall cliff | ✅ done |
| H.8 | Abstention — never return an empty answer | ✅ done |
| H.9 | PII fix — stop dropping UUIDs/SHAs | ✅ done |
| H.10 | `access_log` timestamp bug (range queries returned ∅) | ✅ done |
| H.11 | Delete/wire orphan crates | ✅ done |
| H.12 | `tracemind nightly` reports real signals | ✅ done |
| H.13 | Two-sided CI gate (hash ≈ bge fails) | ✅ done |
| H.14 | Forward-compatible `policy.json` schema | ✅ done |
| H.15 | 5 instrumented design partners | ⬜ todo (human-gated; instrumentation ready) |
| H.16 | Real LoCoMo / LongMemEval data | ⬜ todo (offline-blocked; harness ready) |
| H.17 | Decompose `query()` into declared phases | ⬜ todo (deferred, P3) |
| H.18 | On-device loop as standalone product | ⬜ todo (2027 bet) |

---

## ✅ Done

### F1.2 — Benchmark hygiene / train–test split
Authored `fixtures/locomo-train.json` (45 fresh q) as the tuning split; `locomo-mini.json` (20 q) is held out and only ever scored. `report::embedder_separation_gate` fails CI when a trained encoder doesn't beat `--hash-embed` (the two-sided gate that would have caught the four-release dead-retrieval defect). *Ref: MVP-STATUS §1, H2-AUDIT §2.*

### F1.3 — GEPA wired into runtime, verifier executes
`tm-gepa` rebuilt: the verifier now *executes* anchor queries (the old one read the candidate's self-reported score and could not fail); per-instance Pareto archive (the real mechanism); reflection with textual feedback; system-aware merge. `RetrievalEngine::open()` loads `~/.tracemind/policy.json`; `tracemind policy set/show/reset` promotes a tuned policy; `tm-bench-locomo --gepa` runs the loop. Moved train F1 47.35 → 50.32 over executed rollouts. *Ref: H2-AUDIT §3.*

### F1.4 — Evidence-gated feedback → bandit reward
Un-evidenced queries now register *nothing* (the old model scored every query ~0.1 on the agentic surface, actively mis-training). `HostKind::{Interactive,Agentic}` gates timing signals; `memory_feedback`'s `retrieval_cited`/`retrieval_miss` reach the bandit (were inert rows). *Ref: MVP-STATUS §2.1.*

### H.1 — Core-6 MCP surface
`tools/list` advertises only `memory_store/query/feedback/contradict/compose/forget`; the other 45 stay callable but hidden unless `TM_MCP_ADVANCED=1`. Fewer advertised tools → host picks the right one more often. *Ref: HOLISTIC-REVIEW-IMPLEMENTATION §spine.*

### H.2 — `tm-bench-mcp`
New crate drives the *real* `tm-mcp` binary over stdio JSON-RPC; scores tool-selection accuracy (model-free IDF selector as a host-LLM proxy), latency p50/p95, and contract validity. First time the product was measured as an MCP server.

### H.3 — Sharpen core-6 descriptions
Rewrote descriptions that led with internal sprint codes ("Q4.5 —") to lead with the verbs users say. Measured against the real server: tool selection **55% → 80%**, F1 0.62 → 0.80, p95 51ms.

### H.5 — Retraction beat (the wedge), shipped + measured
`memory_store` now auto-detects, on every store, when a new fact reverses an existing one on a *functional* predicate and surfaces "You told me Carol works at Stripe, but now it's Datadog." Only functional predicates fire (no crying wolf on set membership). Every firing logs to `retractions.jsonl` so the wedge's real trigger rate is countable. *Ref: HOLISTIC-REVIEW-IMPLEMENTATION §differentiators.*

### H.6 — Temporal supersession
The reversed fact's `valid_to` is now closed at store time (`TemporalStore::close_validity` → `GraphStore::supersede_triple`). An as-of query before a reversal returns the old value; after, it doesn't. This makes the beat correct rather than lucky — where the fact-supersession negative result pointed (validity intervals, not a scoring heuristic).

### H.7 — Two-stage ANN, silent cliff removed
Signal search loaded the oldest 2,000 embeddings and silently dropped newer memories (recall decaying with install age). Replaced with a stored 64-bit SimHash prefilter (Hamming rank, no cap) + exact-cosine rerank + recency union. LoCoMo byte-identical (quality preserved, cliff gone).

### H.8 — Abstention
The extractive backend never returns an empty string; it names nearby topics ("nothing on that; closest topics: Alice, Tokyo"). `RetrievalResult` carries `Grounding::{Found,Uncertain,NotStored}`; MCP surfaces the state.

### H.9 / H.10 — Two real bugs behind "flaky" tests
PII filter was silently dropping UUIDs/SHAs/hashes (developer clipboard content — the core use case); now requires letter-free tokens of plausible digit length. Every `access_log` range query returned nothing (space-vs-`T` timestamp string comparison); fixed with RFC3339 writes + legacy normalisation.

### H.11 — Orphan cleanup
Deleted `tm-modal`, `tm-engram`, `tm-bench-wme` (zero dependents/tests). Wired the previously-orphaned `tm-graph::contradiction_rate` into the nightly run. (`tm-cluster`/`tm-pgm`/`tm-temporal` were found used and kept.)

### H.12 — `tracemind nightly` reports real signals
`NightlyScheduler::run()` returned hardcoded `true`s with no work behind them. Now reports real retraction-firing count (from the log) and contradiction rate (from the graph); fabricated flags removed.

### H.13 / H.14 — CI gate + schema durability
`embedder_separation_gate` (H.13) fails when a trained encoder doesn't beat the hash. `policy.json` fields carry serde defaults (H.14) so an upgrade never silently reverts a user's tuned policy.

---

## 🟡 Partial

### H.4 — GEPA over tool descriptions
`tm-bench-mcp --optimize-on` runs the reflective loop with a held-out gate (proposals from one split half, edits kept only if they improve the other) — the same anchor discipline `tm-gepa` uses. **Honest finding:** on a 27-example hand fixture the automated token-harvest can't beat the hand-authored descriptions (trigger vocabulary too diverse for that little data). Built and tested; **waiting on real design-partner utterances** to beat the hand-authored baseline. Blocked by H.16/H.15.

---

## ⬜ To do — retrieval F1

### F1.1 — BGE-M3 dense (768d) end-to-end — RE-SCOPED
The original rationale ("this is why real F1 diverged from bench F1") was **wrong and is retired** — the divergence was the dead retrieval path (hash≡bge because retrieval returned nothing), already fixed. BGE-small-384 is now load-bearing; BGE-M3 is a *further* upgrade, not a bug fix. `BgeM3DenseModel` (768d) exists but is orphaned. **Expected +2–4 F1**; ~1200 LOC (ONNX load, tokenizer, 384→768d migration — SimHash sig regenerates automatically, p50 ≤ 60ms validation). **Gate:** must beat BGE-small on held-out by ≥ 2 F1 *and* hold p50 ≤ 60ms or it isn't worth the footprint.

### F1.5 — Grammar-constrained Tier-1 extractor
Candle Qwen2.5-0.5B backend exists and runs, but output is free-form; no grammar constraint. **Expected +2–3 F1** on structured categories by forcing well-typed spans; ~600 LOC (constrained-sampling mask for candle / GBNF for llama-cpp, per-answer-type schemas). Targets the 4 reasoning-bound questions no GEPA candidate solves.

### F1.6 — Event graph EVG-1/2 completion
Freq-floor edges + `session_scope` join in the planner (charter Q3.3, unfinished). **+2–4 F1** on multi_hop/temporal; ~700 LOC. Substrate for F1.9 and F1.11.

### F1.7 — Multi-query fusion on ComposedIndex
Explicit per-verb paraphrase branching + RRF (N≤3). **+1–2 F1**; ~300 LOC.

### F1.8 — Salience as 5th ComposedIndex space
`salience = f(recency, reuse_rate, verb_affinity_hit_rate)` from the feedback fabric. **+1–2 F1**; ~400 LOC. Needs real usage for `reuse_rate` to matter.

### F1.9 — BridgeRAG multi-hop bridge stage
`PlanAction::Bridge` + `BridgeSpace`. **+2–4 F1** on multi_hop; ~600 LOC. Blocked by F1.6; gate on the held-out multi_hop-vs-single_hop gap on real data.

### F1.10 — Template-parametric GEPA
~20 decomposition/extraction templates + `MutationKind::TemplateSwap`/`TemplatePatch`. **+2–5 F1**, better generalisation; ~800 LOC. Blocked by F1.5.

### F1.11 — RLM depth-2 recursion prototype
Root decomposer → sub-calls over EVG → per-node verifier → budget governor. **+3–8 F1 if the hypothesis holds; biggest lever and biggest risk.** ~1500 LOC. Blocked by F1.6, F1.10. **Run as an experiment**, hard-gated: if flat retrieval already clears 65 F1 on multi_hop on real data, the cost isn't justified.

---

## ⬜ To do — holistic-review track

### H.15 — Five instrumented design partners
Cannot recruit humans in-repo. The **instrumentation is ready**: evidence-gated reward, `retractions.jsonl`, `tracemind nightly` W2-style signals, `tm-bench-mcp`. Metric is W2 retention + whether the retraction beat fires in real sessions — **not F1**. Founder/ops task.

### H.16 — Real LoCoMo / LongMemEval data
Requires downloading external datasets; offline-blocked here. The harness (train/test split, `tm-bench-longmem`, `tm-bench-mcp`) consumes it the moment it's present. **This is the single highest-leverage prerequisite** — 65 hand-written questions bound regressions, not truth; every lift estimate above has an error bar wider than the estimate until measured on real data.

### H.17 — Decompose `query()` into declared phases
The ~1,000-line hot path that produces the tuned 70.49. A declared-phase pipeline would let GEPA search *structure*, not just weights. Deferred (review P3) rather than risk the tuned number in a hasty refactor — which would be the exact "capability outran validation" pattern this whole effort corrected.

### H.18 — On-device self-improvement loop as a standalone product
The trace-log → executed-eval → verified-policy-update → applied-at-runtime loop, generalised past retrieval into a private personalised-policy layer any on-device agent can adopt. 2027 strategic bet, gated on H.15/H.16 evidence.

---

## Do-not-do list (unchanged)

Cross-user population learning · population-based prompt search (HyperAgents) · LoRA/QLoRA sidecar · long-context transformer fallback · 7–8B default Tier-1 (opt-in "beast mode" only) · new embedding model beyond BGE-M3 · re-expanding the advertised MCP surface past the core 6.

---

## Revised execution sequence

- **Sprint 1 — ✅ done:** F1.2 + the reward/robustness/ANN track (H.7–H.14). Trustworthy baseline: 50.32 fresh / 70.49 held-out.
- **Sprint 2 — ✅ done:** F1.3 + F1.4 (loop closed) + H.1–H.6 (surface subtract, MCP bench, sharpen, retraction beat, temporal).
- **Sprint 3 — NEXT:** F1.1 (BGE-M3, gated) + F1.5 (grammar-constrained Tier-1). Target: **55–58 baseline + the 4 reasoning-bound questions**. In parallel, **H.15/H.16 are the real unblock** — get design partners + real data.
- **Sprint 4:** F1.6 (EVG) + F1.7 (multi-query).
- **Sprint 5:** F1.8 (salience) + F1.9 (BridgeRAG, gated on real multi_hop gap). Target: **60+, ready for head-to-head**.
- **Sprint 6:** F1.10 (template GEPA), then F1.11 (RLM) as a hard-gated experiment. H.17 (`query()` decompose) precedes F1.11 if RLM is greenlit.

---

## The one blocker above all code work

**Real data + real users (H.16, H.15).** The harness and instrumentation are built and waiting. Every F1 lift estimate has an error bar wider than the estimate until it's measured on real data, and the two differentiators (retraction beat, composition) have never been validated with a user. Getting the data and the partners is the gating move — a founder/ops task, not a code task.
