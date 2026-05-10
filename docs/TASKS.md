# TraceMind — Task List

**Consolidated**: 2026-05-09. Canonical task list for all implementation work.

Legend: `[x]` done, `[-]` in progress / partial, `[ ]` not started. Est. = estimated F1 lift where applicable.

---

## Shipped (reference)

- [x] Phase 4 / Sprint C-1: bitemporal substrate (tm-temporal embedded into tm-graph; query_at / history; entities + triples backfilled)
- [x] Phase 4 / Sprint C-2: TMS-backed confidence (BeliefStore wraps tm-tms; contradictions surfaced in brief; effective_confidence policy)
- [x] 24-crate Rust workspace compiles and tests pass
- [x] Intent arc types: Need, Sentiment, Action + Belief trait (`tm-intent`)
- [x] Intent arc persistence: SQLite tables + store methods for needs/sentiments/actions
- [x] MCP tools: memory_need, memory_sentiment, memory_action, memory_arc
- [x] CLI commands: need, sentiment, action, arc
- [x] Commitment primitive + state machine + outcome attachment
- [x] CommitmentMiner (implicit phrase mining) + candidate confirm/dismiss
- [x] Daily brief (BriefBuilder) + insights + pattern detector
- [x] World model v0 (f_outcome logistic regression, preflight)
- [x] Outcome proposals (implicit text matcher → accept/dismiss)
- [x] Tier-0 extractive answerer (always-on fallback)
- [x] Tier-1 scaffolded (LocalLlmBackend, Qwen 2.5 1.5B Q4, llama-cpp-2)
- [x] Tier-2 scaffolded (AppleFmBackend stub)
- [x] ColBERT rerank (mxbai-edge-colbert, auto-download)
- [x] BGE-small ONNX embeddings (384-dim)
- [x] 5-arm LinUCB bandit retrieval
- [x] Reasoning: chains, analogy (WL kernel), causal trace, consolidator
- [x] Capture daemon (clipboard + shell history)
- [x] Tauri desktop app shell (basic query/ingest)
- [x] LoCoMo benchmark harness (mini-set, 20 questions)
- [x] LoCoMo v0.2 fixes: speaker-prefix strip, Q→A adjacency, skip-question fallback
- [x] LoCoMo quick wins applied: cosine threshold 0.4→0.2, Q→A window 1→3 turns
- [x] tm-temporal crate (bitemporal facts, TemporalStore, 10 tests)
- [x] tm-tms crate (JTMS engine, BFS propagation, 11 tests)
- [x] tm-modal crate (types, encoder trait, co-occurrence detector, 13 tests)
- [x] tm-engram crate (Engram SDK scaffold, 4 tests)

---

## Priority 0 — Recordable demo (gates everything else)

Sprint C-2 unlocked the *retraction beat* (the demo's hook). Now ship a clean recordable demo before any further engine work. Items are sequential — do not parallelize without explicit redirect.

- [x] **D-1: 3-minute "day in the life" script** — `docs/DEMO_SCRIPT.md`. Five shots; retraction beat in shot 2. Pre-roll uses `tracemind demo restore` + `tracemind demo preroll`.
- [x] **D-2: Pre-warmed demo fixture** — `tracemind demo restore`. ~15 entities, 16 triples, 1 contradiction, 4 open + 5 resolved commitments, deterministic UUIDv5 from a frozen namespace. Sidecar persistence (`memory.db.contradictions.json`) so the contradiction survives across CLI invocations.
- [x] **D-3: Real ambient capture in demo path** — `tracemind demo preroll [--seconds N]` spawns the existing `tracemind-capture` daemon silently (stdout/stderr suppressed) for the pre-roll window, then reaps it. Resolves the binary via `$TM_CAPTURE_BIN` → sibling executable → PATH.
- [x] **D-4: Brief renders in the Tauri app** — new `cmd_brief` IPC + `BriefView.tsx` render the same DailyBrief the CLI shows, with the contradictions row at the top. `Brief` is the new default landing tab. The `models/**/*` glob is satisfied by the existing `models/manifest.json` placeholder.
- [x] **D-5: Single-binary install** — `scripts/install.sh`. Detects platform, downloads release tarball, optional SHA256 verification, installs `tracemind` / `tm-mcp` / `tracemind-capture` to `/usr/local/bin` (or `~/.local/bin`), creates `~/.tracemind/`. Idempotent.
- [x] **D-6: One-screen product close** — `docs/PRODUCT_CLOSE.md`. Three products / one engine: TraceMind (personal memory OS), Engram (memory SDK), Rosetta (semantic code memory). Shared crates listed; install one-liner on the end card.

**Why P0:** items 1–4 unlock the recorded demo. 5–6 are needed before screen-sharing to anyone outside.

---

## Priority 0.5 — Context segmentation (Sprint C-0, NEW 2026-05-10)

**Wedge-critical.** Investor review on 2026-05-10 flagged the demo's cross-document bridge (Rondo↔TraceMind) as a *misfeature*: local machines have more context crowding than cloud (one laptop hosts Sidewalk, Horseshoe, Rondo, TraceMind, personal life). Without context discipline, a local memory OS is strictly worse than separate cloud accounts. The pitch is "system of intents + trust on-device" — and trust collapses the first time TraceMind draws an irrelevant parallel.

**Design shape:** decoupled-by-default, opt-in coupling via accumulated positive signal, negative-feedback first-class.

Ordering: blocks both Tier-1-as-default and Sprint-D demo polish. Do C-0 first, then return to Priority 1.

- [x] **C-0.1 Schema (landed 2026-05-10)** — `contexts` + `negative_signals` tables and additive `context_id` column on `captured_signals` are created on every `GraphStore::open` via `tm_graph::context::init_schema`. Idempotent (re-runs are no-ops). `kg_relations` / `entities` will carry `context_id` inside their skg JSON `properties` blob (next slice — keeps skg's schema untouched). Migration of legacy stores is implicit: pre-existing rows have NULL `context_id`, which the retrieval filter will always treat as "always visible".
- [x] **C-0.2 Active-context state (landed)** — `~/.tracemind/active_context.json` with atomic tempfile-rename writes; `ActiveContext::load / save / clear` in `tm-graph::context`.
- [x] **C-0.3 Context CRUD (landed)** — `GraphStore::{create_context, list_contexts, get_context_by_name, write_negative_signal, negative_weight_for_query}`. 5 unit tests pass (`cargo test -p tm-graph context::`).
- [x] **C-0.4 CLI surface (landed)** — `tracemind context create <name> [--tags t1,t2]`, `context list` (marks active with `*`), `context use <name>`, `context current`, `context clear`. Smoke-tested end-to-end.
- [x] **C-0.5 Ingest tagging (landed 2026-05-10)** — `IngestPipeline::open` loads `ActiveContext` from `<data_dir>/active_context.json` (sibling of `memory.db`) and calls `graph.set_active_context(...)` once. `GraphStore::upsert_entity`, `upsert_triple`, `log_signal`, and `insert_signal_with_embedding` all read the borrowed `active_context_id` and tag new rows: entities and triples stash the UUID in their skg `properties` JSON; `captured_signals` writes the dedicated `context_id` column. Round-trip tests for entity / triple / signal tagging + unscoped (NULL) inheritance pass (`cargo test -p tm-graph store::tests::{entity_tagged_with_active_context, entity_untagged_when_no_active_context, triple_tagged_with_active_context, signal_tagged_with_active_context, entity_in_active_scope_includes_unscoped}`).
- [x] **C-0.6 Scoped retrieval (landed 2026-05-10)** — `RetrievalEngine` carries a `cross_context: bool` field (default `false`) with `set_cross_context(bool)` setter. After per-query entity / triple / signal loading, results are post-filtered: rows whose `context_id` differs from `graph.active_context_id()` are dropped; unscoped (NULL) rows always pass. Triples are additionally pruned if either endpoint was filtered out. CLI surfaces `--cross-context` on both `tracemind query` and `tracemind ask`; MCP `memory_query` accepts `cross_context: boolean` (default false). Integration test in `tm-retrieval::engine::tests::retrieval_scopes_to_active_context` covers both modes.
- [x] **C-0.7 Cross-context penalty + reward decomposition (landed 2026-05-10)** — two things land together so the negative-feedback loop actually closes. (1) After the rerank phase, when `cross_context=true` and an active context is set, candidates whose entity carries a *foreign* `context_id` take a soft `-0.15` score adjustment and the candidate list is re-sorted. Unscoped (legacy NULL) rows and same-context rows are untouched. (2) Every `query()` mints a `query_id: Uuid` that the retrieval trace, the new `PendingReward.query_id`, and the new `RetrievalResult.query_id` field all share — so `tracemind not-related <query_id> <result_id>` ties back to the originating bandit pull. `finalize_pending_reward` now reads `graph.negative_weight_for_query(pending.query_id)` and clamps `final_reward = (relevance_reward - Σ negative_weights)` into `[0, 1]` before calling both `UcbBandit::register_reward` and `LinUcbBandit::register_reward`. Tests: `tm-retrieval::engine::tests::{not_related_signal_subtracts_from_bandit_reward, cross_context_penalty_reorders_candidates}`.
- [x] **C-0.8 Negative feedback CLI (landed 2026-05-10)** — `tracemind not-related <query_id> <result_id> [--weight w] [--kind k] [--context-a u] [--context-b u]` writes a `negative_signals` row. Separate top-level command (not `feedback --not-related`) to preserve the existing positional `feedback <arm> <reward>` API. Reward decomposition helper (`tm_graph::negative_weight_for_query`) ships; consuming it in `UcbBandit::register_reward` is wired in the retrieval-filter slice. MCP equivalent (`memory_feedback {kind: "not_related"}`) still pending.
- [ ] **C-0.9 Brief + UI surfacing** — daily brief header shows the active context; per-row context tag rendered next to each result; Tauri brief view picks this up via existing `cmd_brief`.
- [x] **C-0.10 Demo update (landed 2026-05-10)** — `scripts/demo_real.sh` fully rewritten. Shots 1 + 2 now create + activate named contexts (`rondo`, `tracemind`) before ingest, so every entity / triple / signal is tagged at write time. Shot 6 is the new scoped-recall flagship: the *same* query "the bet on running the brain on-device" is run once in each context and returns two different, properly scoped answers — explicitly framing context blur as a misfeature instead of celebrating it. A new shot 8 (replacing the old "cross-document bridge" shot) demonstrates the full negative-feedback loop end-to-end: an opt-in `--cross-context` query surfaces a bridge, the demo parses the printed `Query: <uuid>` line, files `tracemind not-related <query_id> <result_id> --kind cross_context_bridge`, then a follow-up query finalises the pending reward so the bandit consumes the penalty. Smoke-tested clean against the rebuilt release binary. **Sub-fix that landed with this slice:** `RetrievalEngine::open` now reads `<data_dir>/active_context.json` the same way `IngestPipeline::open` has since C-0.5 — without this, the CLI / MCP query path never picked up the user's active scope, so C-0.6's filter was dormant outside tests.
- [-] **C-0.11 Tests** — context CRUD + active-context-file + negative-signal sum tests landed (5 in `tm-graph::context::tests`). Ingest-writes-context-id tests landed for entity / triple / signal (`tm-graph::store::tests`) plus an end-to-end retrieval-scope test in `tm-retrieval::engine::tests` (active scope filters foreign entities, `cross_context=true` bypasses). Reward-decomposition + cross-context penalty tests landed (2 in `tm-retrieval::engine::tests`). Schema-migration test (legacy DB → migrated DB with NULL context_ids) still pending.

**Why P0.5:** The L3 / recommendation surface is only valuable when cross-context parallels are *real*. Without C-0, the LLM amplifies bad bridging. C-0 is foundational to every later tier.

---

## Priority 0.7 — Feedback-driven self-improvement loop (GEPA + MIPRO)

**Wedge claim:** every memory product gets better by scaling *capture*; we get better by scaling *feedback*. C-0.7 closed the negative-feedback loop end-to-end (user files `not-related`, bandit reward is decomposed); this priority generalises that into a full on-device optimizer. Full design in `docs/FEEDBACK_LOOP.md`.

- [-] **F-1 Positive signal CLI + storage (CLI + Tauri landed 2026-05-10)** — new `positive_signals(id, query_id, result_id, kind, context_id, weight, created_at)` table; `GraphStore::{write_positive_signal, positive_weight_for_query}`; `tracemind helpful <query_id> <result_id>` CLI + Tauri `cmd_helpful`; `finalize_pending_reward` composes `(relevance + Σ positives - Σ negatives).clamp(0, 1)`. Tests: 2 added. **MCP `memory_feedback {kind: "helpful"}` is the remaining sub-item** — adds a single dispatcher branch in `tm-mcp` that calls `GraphStore::write_positive_signal`.
- [ ] **F-2 Corpus extractor** — new `tm-eval::corpus` module. Reads `traces.jsonl × {positive_signals, negative_signals × recent.jsonl}` and materialises a typed `EvalCorpus { examples: Vec<EvalExample> }`. Dedup on `(query_text, ts_bucket)`. Excludes ingest traces. Idempotent test. ~1 day.
- [ ] **F-3 Parameter surface (`TuneConfig`)** — move ~20 named knobs (planner thresholds, UCB1 / LinUCB exploration, rerank α, RRA weights, MMR λ, `cross_context_penalty`, low-confidence threshold, extractive template set) from inline constants into `tm-types::TuneConfig`. Load from `~/.tracemind/configs/active.toml` (default shipped with the release). CLI `tracemind config {show, edit, rollback, diff <id>}`. ~2 days.
- [ ] **F-4 `tm-eval` crate** — replays an `EvalCorpus` against a given `TuneConfig`, returns per-axis scores `EvalReport { f1, em, engagement_reward, neg_signal_rate, latency_p50, latency_p95 }`. Wire LoCoMo bench harness as one of the input corpora so we can co-optimise on user feedback *and* the public benchmark. ~1.5 days.
- [ ] **F-5 `tm-tune` crate (GEPA-lite, on-device)** — idle-time binary that maintains a pareto frontier of `TuneConfig` candidates per query class. Mutation menu: numeric jitter, threshold step, extractor template swap, demo curation (Tier-1 gated). CLI `tracemind tune {run [--budget-min 10], frontier, promote <id>}`. Auto-promote is **off by default** — user must `promote` to swap `active.toml`. Atomic-symlink rollback. ~3 days.
- [ ] **F-6 MIPRO-style few-shot demo selection (Tier-1)** — once `local-llm` is on, extend the mutator with "demo swap" actions: choose 3 examples for the Tier-1 prompt from the user's high-positive corpus, optimised per-task-kind. Joint search over (instruction, demos). ~2 days, blocked on Tier-1 wiring.

**Why P0.7:** The Tier-1 LLM and the L3 prediction surface both ride on the same retrieval stack. Making the stack *learn from the user's own corrections* is the moat — it is the one capability cloud competitors can never copy without uploading the feedback corpus.

---

## Priority 0.8 — Investor demo UI sprint (Sprint D, NEW 2026-05-10)

**Why P0.8:** The Sprint C engine work is invisible from the CLI demo — investors can't *see* context scoping, retraction, or the closing feedback loop. The Tauri shell already has 27 commands + a 354-LoC API surface and BriefView landing tab; we need 4 surgical additions to make the demo *feel* like a product. Ordering matters — each item is the smallest unit that produces a visible-in-screen-record win.

- [-] **UI-1 Branch + Tauri build sanity (landed 2026-05-10)** — branch `sprint-d-investor-ui` cut from `2828160`. `cargo build -p tm-tauri` + `cargo build --workspace` both clean — the `models/**/*` glob block referenced in older memory is no longer present.
- [x] **UI-2 `query_id` on Tauri `QueryResponse` (landed 2026-05-10)** — `QueryResponse` now carries `query_id: String` straight from `RetrievalResult.query_id`. Frontend `QueryResponse` interface in `crates/tm-tauri/ui/src/api.ts` mirrors it. Inline buttons (UI-5) consume it.
- [x] **UI-3 Active-context badge + switcher (landed 2026-05-10)** — new Tauri commands `cmd_context_list`, `cmd_context_current`, `cmd_context_use`, `cmd_context_create`, `cmd_context_clear`. Frontend `ContextSwitcher.tsx` renders a sidebar dropdown showing the active context, lists every context, allows inline switch + create. `cmd_context_use` hot-swaps the live `RetrievalEngine`'s active context via a new `set_active_context(&mut self, Option<Uuid>)` method on the engine (graph already used interior mutability). No app restart required.
- [x] **UI-4 F-1 backend: `positive_signals` + `helpful` CLI/MCP (CLI landed 2026-05-10)** — new `positive_signals(id, query_id, result_id, kind, context_id, weight, created_at)` table in `tm_graph::context::init_schema`. `GraphStore::{write_positive_signal, positive_weight_for_query}` mirror the negative-signal helpers. `tracemind helpful <query_id> <result_id> [--weight w] [--kind k] [--context-id u]` CLI command writes a row (default weight 0.3). `finalize_pending_reward` now composes `(relevance + Σ positives - Σ negatives).clamp(0, 1)` so both channels feed the bandit symmetrically. Tests: `tm_graph::context::tests::positive_signals_sum_correctly` + `tm_retrieval::engine::tests::helpful_signal_adds_to_bandit_reward`. **MCP `memory_feedback {kind: "helpful"}` still pending** — Tauri `cmd_helpful` ships with this slice, MCP surface deferred to the next slice.
- [x] **UI-5 Inline 👍 / 👎 / wrong-ctx buttons on QueryView rows (landed 2026-05-10)** — per-entity row now renders three icon buttons. 👍 → `cmd_helpful(query_id, entity_id)` (weight 0.3, kind `helpful`); 👎 → `cmd_not_related(... kind="not_related")` (weight 1.0); "wrong ctx" → `cmd_not_related(... kind="cross_context_bridge")` (weight 1.0). UI is optimistic: row dims and replaces the buttons with an "noted ✓" / "filed not-related" / "filed wrong-context" pill on click, rolls back on backend error. Stored `rowFeedback` map clears on every new query.
- [x] **UI-6 Demo fixture restore (landed 2026-05-10)** — `tracemind demo restore` runs cleanly against the C-0 schema. `init_schema` creates the new `contexts` / `positive_signals` / `negative_signals` tables on every open, so no migration drift. Extended the fixture to seed **two deterministic contexts** (`Mercury work`, `TraceMind dev`) keyed by UUIDv5 from the frozen `DEMO_NAMESPACE`. The 15 entities + 16 triples are now partitioned across the two scopes (8/7 entities, 9/7 triples — Alice/Bob/Mercury + the loves/hates contradiction live in Mercury work; demo project + Postgres/SQLite + investor pitch live in TraceMind dev). Verified: brief still renders with stable contradiction short IDs (`b6160aa5 ↔ 1b7b36b3`); `tracemind context list` returns both rows; `tracemind context use "Mercury work"` swaps the active scope; second `tracemind demo restore --force` is idempotent (still 2 contexts / 15 entities).
- [-] **UI-7 End-to-end smoke + recording (smoke + docs landed 2026-05-10; recording still pending)** — new `scripts/demo_smoke.sh` (11 checks) drives the full flow from the CLI: `demo restore` → `context list` → scoped ingest into each context → scoped query → `tracemind helpful $QID alice` (verifies `positive_signals` row lands) → `tracemind not-related $QID postgres --kind cross_context_bridge` (verifies `negative_signals` row lands) → `brief` re-renders with the contradiction + commitments intact. All 11 checks pass against `release` build. `docs/DEMO_SCRIPT.md` rewritten: context-segmentation beat is now **Shot 2** (0:25 – 1:00), retraction beat moves to Shot 3, clipboard grounding → Shot 4, outcome prompt → Shot 5, product close → Shot 6. Hard-requirements section adds two new bullets (UI-3 switcher visible, UI-5 buttons write signals). **Remaining sub-item: the 60-90s screen recording itself** — needs a human capture session against the Tauri app with screen capture + voiceover.

**Out of scope for Sprint D** (deferred to Sprint E): L2/L3 surfaces, force-directed memory garden, voice capture, calibration panel. Those are P3/P4 — this sprint is *only* the engine surfaces that already exist.

---

## Priority 1 — Quality (highest F1 impact, pure code work)

### LoCoMo Tier-0 remaining fixes

- [ ] **Heuristic NER for span extraction** — dates, money, named entities, percentages. Return the span ("April 20", "$2.5M") instead of the full sentence. (Est. +8–15 F1)
- [ ] **Yes/no oracle** — pre-screen `Did/Was/Is/Has` questions; if predicate disagrees with retrieved evidence, emit "No" + the contradicting fact. Tier-0 extractive cannot do this today. (Est. +5–10 F1)
- [ ] **Recency bias for duplicate-entity turns** — when multiple turns mention the same entity, prefer the latest mention. (Est. +3–5 F1)

### Tier-1 as default

- [ ] **Make Tier-1 the shipping default** — today it only activates with `--features local-llm`. Needs: first-run download flow with progress UI, auto-download on first query when weights missing, Tier-1 dispatched for all synthesis (not just structured tasks).
- [ ] **Run LoCoMo mini-set with Tier-1** — measure actual F1 gain from LLM synthesis vs. extractive. Gate: ≥60 F1.

---

## Priority 2 — Wire isolated crates into the pipeline

### Wire tm-tms into ingest + retrieval (Sprint C-2 — mostly shipped)

- [x] Call `TmsEngine.assert_belief()` at ingest time when entities/triples are upserted
- [x] Contradiction detection at ingest (cosine < -0.8 via tm-tms threshold; schema/temporal triggers still pending)
- [x] Belief-aware retrieval ranking: hide `Out`, downrank `Contradicted` via `effective_confidence`
- [x] Surface contradictions in daily brief via tm-reflect
- [ ] Schema-constraint and temporal-overlap contradiction triggers (in addition to cosine)
- [ ] Persist TMS state across restarts (currently rebuilt from live triples)

### Wire tm-temporal into tm-graph (Sprint C-1 — shipped)

- [x] Bitemporal substrate via embedded `TemporalStore` (sibling DB); write-through on every entity/triple upsert
- [x] `GraphStore::entity_at`, `triple_at`, `entity_history`, `triple_history`
- [ ] `belief_revisions` table for explicit retraction provenance (separate from JTMS retractions)
- [ ] Time-machine queries in CLI + MCP: "what was I thinking in March?"
- [ ] `GraphStore::diff(from, to)` for change inspection

---

## Priority 3 — Product (session quality + UX)

### WorkingMemory ring buffer

- [ ] Define `tm-types::WorkingMemory` — Vec-backed in-RAM ring of last N turns + retrieval results
- [ ] Feed working memory into subsequent queries (session context for follow-ups)
- [ ] Include working memory in `ContextSnapshot` at commitment time

### NarrativeResponse

- [ ] Define `NarrativeResponse` in tm-types: `text`, `citations`, `related_threads`, optional `surprise`, optional `voice_audio`
- [ ] Replace raw `RetrievalResult` / `AnswerResponse` at all user-facing boundaries (CLI, MCP, Tauri)

### Query rewriting

- [ ] Tier-1 paraphrase expansion: expand raw question into 3 paraphrases, run all through retrieval, RRA fusion. (Est. +5–8 F1 on multi-hop)

### Auto-detection pipeline components

- [ ] `NeedDetector` in tm-capture — mine "I need to", "problem is", "goal is" patterns from capture stream
- [ ] `SentimentScorer` — heuristic valence scoring from captured text (keyword-based first)
- [ ] `ActionMatcher` — match shell commands, file edits, git commits to open commitments via embedding similarity

---

## Priority 4 — Tauri UI surfaces

- [ ] Daily brief panel (read on open, dismissable, archived)
- [ ] Commitment timeline (vertical, color-coded by state, click → drawer with full context)
- [ ] Intent arc visualization (Need → Sentiment → Commitment → Action → Outcome graph)
- [ ] Memory garden (force-directed entity graph)
- [ ] Capture timeline (chronological ingest view)
- [ ] "What I noticed" surprise panel
- [ ] First-run onboarding (sample data → meaningful brief in 60 seconds)
- [ ] Settings panel (personality, voice, brief schedule)
- [ ] Calibration panel (predictions made, outcome accuracy, Brier score, pattern stats)

---

## Priority 5 — Voice + capture moat

- [ ] `tm-voice` crate — Whisper-tiny STT (39MB) + Piper TTS (60MB)
- [ ] Global hotkey ⌘⇧Space → hold-to-record → Tier-1 normalizes to Commitment draft
- [ ] Daily brief TTS playback
- [ ] Global hotkey ⌘⇧M for quick capture
- [ ] Screenshot capture + VLM caption (SigLIP-small for encoding, Moondream/Phi-3.5-vision for caption)
- [ ] Obsidian vault import (walk all Markdown files, ingest)
- [ ] Browser extension (capture active tab content)

---

## Priority 6 — Engine depth (Phase 5A)

### Cross-modal pipeline

- [ ] Implement SigLIP-small ONNX encoder for images
- [ ] Implement tree-sitter + BGE code encoder
- [ ] Wire `ModalIngestPipeline` into tm-ingest (co-occurrence edges within 30-second window)
- [ ] Add arm 5 (cross-modal) to tm-controller: top_k=12, hops=1, cross-modal=yes
- [ ] Cross-modal chains in tm-reason (`CrossModalChainBuilder`)
- [ ] Cross-modal citations in synthesis (screenshots + code spans)

### World model upgrades

- [ ] `f_topic` MLP (2-layer, 384→512→384, InfoNCE) — powers L1 silent prefetch
- [ ] L1 silent prefetch: background task pre-warms tm-retrieval based on f_topic predictions
- [ ] Platt scaling / isotonic regression on f_outcome for calibrated probabilities
- [ ] Extend LinUCB context vector with working-memory state, tier, affective-graph density

### Nightly processes

- [ ] `tm-reflect` nightly cron/daemon (scheduled brief generation)
- [ ] TMS consistency check nightly
- [ ] Deductive/inductive promotion (raw captures → graph entities)
- [ ] Temporal GC (archive retracted facts older than N days)
- [ ] Two-speed ingestion: signal lake (fast, raw) + graph promotion (background)

---

## Priority 7 — Engram extraction (Phase 5B)

- [ ] Finalize Belief trait API across all intent-arc types
- [ ] `tm-engram` full implementation: assert/retract/world_at/contradictions/history_of/set_goal/record_action/observe
- [ ] MCP tools: memory_believe, memory_retract, memory_world_at, memory_contradictions
- [ ] Standalone Engram MCP server binary
- [ ] Python wrapper (PyO3) → PyPI: `engram`
- [ ] TypeScript wrapper (napi-rs) → npm: `@tracemind/engram`
- [ ] Integration test with Claude Code
- [ ] Publish to crates.io
- [ ] Benchmark: assertion latency <1ms, retraction propagation <10ms for 10k beliefs
- [ ] 3 example agents (research assistant, code review, customer support)

---

## Priority 8 — Rosetta foundation (Phase 5C)

- [ ] `tm-semcode` crate: tree-sitter parsing (Rust, Python, TypeScript, Go, Java)
- [ ] Intent extraction pipeline: code → AST analysis → cross-modal fusion with tests/docs/git/PRs → CodeIntent
- [ ] `SemanticDiff` engine: semantic change classification (Cosmetic/Refactor/IntentShift/New/Deleted)
- [ ] Intent drift detection (bitemporal over git history)
- [ ] `tm-rosetta` CLI: `rosetta diff HEAD~5..HEAD`
- [ ] MCP tools: code_intent, code_semantic_diff
- [ ] VS Code extension stub
- [ ] (Stretch) Intent-preserving refactoring generation + property-based test generation

---

## Priority 9 — Mobile + sync (Phase 6)

- [ ] UniFFI bindings (Swift + Kotlin)
- [ ] iOS app (SwiftUI)
- [ ] Android app (Compose)
- [ ] Apple FoundationModels backend (macOS 26+)
- [ ] Photo ingest (EXIF + VLM caption)
- [ ] `tm-sync` crate: Automerge CRDTs over iCloud/Drive
- [ ] End-to-end encryption with per-user keypair
- [ ] Opt-in encrypted-cloud Tier (low-end devices only)

---

## Priority 10 — Scale + world model v2 (Phase 7)

- [ ] `f_outcome` v2: 4-layer transformer (~3M params, d=256, 4-class polarity)
- [ ] HNSW vector index for 100k+ entities (replace flat scan)
- [ ] Mamba/SSM history compression
- [ ] GraphSAGE GNN for AnalogySolver (replace WL kernel)
- [ ] Louvain/Leiden community detection in Consolidator
- [ ] Factorization machine / MLP for PatternDetector (cross-cell interactions)
- [ ] `tm-preference` crate: triplet contrastive loss on (query, kept_answer, rejected_answer)
- [ ] Loop 4 counterfactual replay (doubly-robust OPE over trajectory store)
- [ ] Iterative/agentic retrieval (multi-step query refinement)
- [ ] Context-budget allocator (dynamic retrieval budget per query complexity)

---

## Evaluation infrastructure

- [ ] Full LoCoMo run (~7,000 questions) — mini-set numbers not comparable to competitors
- [ ] Cross-modal eval harness: 50 hand-labeled multi-modal reasoning scenarios
- [ ] Bitemporal correctness test suite (formal, 100%)
- [ ] Intent preservation eval: 100 labeled before/after refactoring pairs
- [ ] Belief consistency eval: formal JTMS spec compliance
- [ ] Performance benchmarking: idle RAM, active RAM, cold-query latency, Tier-1 hot-query latency

---

## Priority 11 — LLM packaging + on-device personalization (deferred)

Lower-priority phase queued behind the recordable demo and the existing P1–P10 work. Two layers:

### L1 — `tm-llm` packaging crate

- [ ] New `tm-llm` crate: `ModelManifest` (sha256, size, prompt template, tokenizer hash, license) + `ModelRegistry` reading `~/.tracemind/models/manifest.toml`
- [ ] First-run model fetch with checksum verification + atomic install (no half-downloaded weights)
- [ ] LoRA adapter slot: `BaseModel + Vec<AdapterSpec>` with hot-swap at the `LocalLlmBackend` boundary
- [ ] CLI surface: `tracemind models list / install / remove / verify`
- [ ] MCP surface: `model_status` returning manifest + adapter state

### L2 — Resource-constrained on-device finetune

- [ ] Python sidecar (`tools/finetune/`) using transformers + peft + bitsandbytes (Linux/Win) or MLX-LM (macOS) for QLoRA
- [ ] Three default LoRA roles: `summarizer-personal`, `extractor-personal`, `prefs-personal`
- [ ] Training data builder: pulls from accepted/rejected edits in tm-trace + commitment outcome history
- [ ] Nightly schedule: opt-in only, runs when on AC + idle, capped at 30min wall clock
- [ ] LoRA weights stay in `~/.tracemind/adapters/` — never leave device, no telemetry
- [ ] Lightweight TRL/Unsloth alternative path for low-RAM machines (8GB target)
- [ ] CLI: `tracemind finetune status / start / stop / rollback`
