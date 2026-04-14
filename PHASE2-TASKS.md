# TraceMind Phase 2 — Task Board

Status: `TODO` | `IN_PROGRESS` | `DONE` | `BLOCKED`

---

## TM-P2-001 — Graph store + tracing
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** none
**What:** Originally planned Kuzu rewrite, but Kuzu had fatal build issues (cxx version mismatch, cmake dependency). Decision: keep rusqlite, add tracing. Graph store works, 5 tests pass.
**Accept:** `cargo test -p tm-graph` passes. ✅

---

## TM-P2-002 — Update upstream crates for new Embedder API
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-001
**What:** `Embedder::new()` now returns `Result<Self>`. Updated `tm-ingest`, `tm-retrieval` to handle this. Tests use `Embedder::new_hash()` and temp dirs for LanceDB. VectorStore path is now a directory (LanceDB) not a `.vec` file.
**Accept:** `cargo build --workspace` compiles. `cargo test --workspace` passes (42/42). ✅

---

## TM-P2-003 — End-to-end smoke test with real backends
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** `tracemind ingest` / `query` / `trace` / `status` work with SQLite + LanceDB + fastembed hash embedder. Data persists across CLI calls. All 4 commands verified on temp data dir.
**Accept:** 4-command smoke test passes on a temp data dir. ✅

---

## TM-P2-004 — Real ONNX embeddings (fastembed model download)
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-003
**What:** `Embedder::new()` loads real all-MiniLM-L6-v2 via fastembed. Added `--hash-embed` global CLI flag and `TM_HASH_EMBED=1` env var for MCP. `IngestPipeline::open()` and `RetrievalEngine::open()` accept `hash_embed: bool`.
**Accept:** `tracemind ingest "hello"` uses real 384-dim embeddings. `--hash-embed` flag works. ✅

---

## TM-P2-005 — Procedural memory executor
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `ProcedureStore` (JSONL, dedup by id) and `dry_run()` to tm-episodic. CLI commands: `proc add`, `proc list`, `proc run` (dry-run), `proc feedback --success/--fail`. Lifecycle: Active → Reinforced → Degraded → Deprecated. 5 unit tests.
**Accept:** Unit tests pass. CLI `proc` commands work. ✅

---

## TM-P2-006 — Confidence decay
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `GraphStore::decay_all(factor, threshold)` method. `tracemind decay` CLI command with `--factor` and `--threshold` flags (defaults 0.95/0.05). Unit test passes. MCP timer deferred to Tauri phase.
**Accept:** Unit test passes. CLI `decay` command works. ✅

---

## TM-P2-007 — Persistent bandit across all CLI commands
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `UcbBandit::load/save` to tm-controller. RetrievalEngine auto-loads/saves bandit.json. CLI Feedback/Status commands use load/save. Removed ~50 lines of duplicated persistence code from CLI.
**Accept:** Run 5 queries, `tracemind status` shows correct pull counts. ✅

---

## TM-P2-008 — Tauri desktop app scaffold + IPC
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-003
**What:** `cargo tauri init` in workspace. Tauri commands wrapping: ingest, query, trace, status, decay. React + TypeScript frontend shell with sidebar nav. No UI content yet — just the plumbing.
**Accept:** `cargo tauri dev` opens a window. Calling ingest from JS IPC returns entity count.

---

## TM-P2-009 — Tauri UI: query + dashboard
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-008
**What:** Dashboard: entity count, triple count, trace count, bandit arm stats. Query view: text input, entity cards, triple list with names. Tailwind CSS.
**Accept:** Can ingest text and query it through the UI.

---

## TM-P2-010 — Tauri UI: memory timeline + entity graph
**Status:** DONE
**Branch:** `claude/brave-herschel`
**Deps:** TM-P2-009
**What:** Force-directed graph visualization on HTML5 canvas. Responsive sizing (ResizeObserver + devicePixelRatio). Type filter dropdown, label toggle, degree-based node sizing. Adaptive physics for large graphs (300+ nodes) with spatial cutoff. Color-coded entity types with legend. Drag-to-reposition nodes. Hover info panel showing connections.
**Accept:** Can see full knowledge graph rendered with entity types color-coded. Can filter by type, toggle labels, drag nodes. ✅

---

## TM-2.6 — Demo Polish Sprint
**Status:** DONE
**Branch:** `claude/brave-herschel`
**Deps:** TM-P2-010

### TM-2.6-001 — Entity deduplication
**Status:** DONE
**What:** Case-insensitive name matching during ingest. Reuses existing entity UUIDs, reinforces confidence (+0.05). Within-batch dedup by lowercase name.
**Accept:** Ingesting "Rust" twice doesn't create duplicates. ✅

### TM-2.6-002 — Explicit feedback (thumbs up/down)
**Status:** DONE
**What:** `explicit_feedback(score)` on RetrievalEngine registers reward on current bandit arm. Frontend +/- buttons in QueryView meta bar.
**Accept:** Clicking +/- updates bandit state. ✅

### TM-2.6-003 — PageRank-weighted recommendations
**Status:** DONE
**What:** Recommendations scoring formula: 0.4*relevance + 0.2*recency + 0.2*novelty + 0.2*pagerank. Cold-start: 0.3*recency + 0.2*novelty + 0.2*confidence + 0.3*pagerank.
**Accept:** High-PageRank entities surface in recommendations. ✅

### TM-2.6-004 — Community coloring (Louvain)
**Status:** DONE
**What:** GraphStore wraps `sqlite-knowledge-graph` Louvain. `cmd_graph` populates community per node. Frontend toggle between type/community coloring with 12-color palette.
**Accept:** Graph view shows community clusters with distinct colors. ✅

### TM-2.6-005 — "Surprising entities today" widget
**Status:** DONE
**What:** `cmd_surprising` returns high-novelty recently captured entities. Dashboard shows "Most Surprising Today" widget with novelty/recency percentages.
**Accept:** Dashboard shows surprising entities. ✅

### TM-2.6-006 — Batch SQL scoring
**Status:** DONE
**What:** Replaced N+1 `recency_score()`/`novelty_score()` calls with `batch_recency_scores()`/`batch_novelty_scores()` single-query batch fetches.
**Accept:** No performance regression; same results. ✅

### TM-2.6-007 — Entity trend sparklines
**Status:** DONE
**What:** `cmd_entity_trends` returns entities created per day over last 7 days. Dashboard shows bar chart sparkline of entity growth.
**Accept:** Dashboard shows 7-day trend. ✅

### TM-2.6-008 — Entity delete from graph
**Status:** DONE
**What:** `cmd_delete_entity` deletes entity + relations + vectors + access logs. Right-click context menu on graph nodes with "Delete Entity" option.
**Accept:** Right-click delete removes entity and refreshes graph. ✅

---

## Execution order

```
P2-001 (graph+tracing) ✅
  → P2-002 (api fix) ✅
    → P2-003 (e2e smoke) ✅
      → P2-004 (real embed) ✅
      → P2-007 (bandit persist) ✅
    → P2-005 (procedural) ✅
    → P2-006 (decay) ✅
  → P2-008 (tauri scaffold) ✅
    → P2-009 (tauri query UI) ✅
      → P2-010 (tauri graph viz) ✅
        → TM-2.6 (demo polish) ✅
          → TM-3.0 (local reasoning) ✅
            → TM-3.1 (MIA retrieval intelligence) ✅
              → TM-3.2 (R1-inspired architecture) ✅
              → TM-3.3 (LinUCB + attenuation + reward) ✅
                → TM-3.4 (decomposition + procedures + uncertainty + diversity + trajectory prior) ✅
                  → TM-3.5 (BIGMAS workspace + MEM ingestion gate + temporal decay) ✅
                    → TM-4.0-001 (ONNX embeddings + model selection + benchmark) ✅
```

---

## Phase 3.0 — Local Reasoning Engine

### TM-3.0-001 — Graph-of-Thought reasoning chains
**Status:** DONE
**What:** New `tm-reason` crate with `ChainBuilder`. Multi-hop BFS traversal that builds scored reasoning paths between entities. Excludes noisy RelatedTo edges. Hop decay factor (0.85^n) prefers shorter paths. `explore()` method for open-ended reasoning from seed entities. 3 tests.
**Accept:** `cargo test -p tm-reason` passes. `cmd_reason_chain` and `cmd_reason_explore` IPC commands work. Frontend "Reason" view with Explore/Chain/Analogy modes. ✅

### TM-3.0-002 — Causal tracing / attribution
**Status:** DONE
**What:** `CausalTrace` struct records how each entity was discovered: vector match, graph hop, episodic trace, or reasoning chain. `explain()` generates human-readable attribution. `top_attributions(n)` returns highest-weight evidence. 2 tests.
**Accept:** Causal traces can be built and explained. ✅

### TM-3.0-003 — Analogical reasoning (WL kernel)
**Status:** DONE
**What:** `AnalogySolver` computes entity neighborhood fingerprints using 1-hop and 2-hop predicate patterns (simplified Weisfeiler-Leman). Jaccard similarity between fingerprints. Same-type bonus. `cmd_find_analogies` IPC command. 1 test.
**Accept:** "What's like Rust?" finds Python (both Technologies with DependsOn→Project pattern). ✅

### TM-3.0-004 — Memory consolidation ("sleep")
**Status:** DONE
**What:** `Consolidator` runs Ebbinghaus forgetting curves: strengthens frequently-accessed entities, decays old ones, prunes below threshold, merges near-duplicate names. `cmd_consolidate` IPC. Dashboard "Sleep" button. 3 tests.
**Accept:** Consolidation merges duplicates, prunes weak entities. ✅

### TM-3.0-005 — find_entity_by_id
**Status:** DONE
**What:** Added `GraphStore::find_entity_by_id(Uuid)` for direct ID lookup (used by reasoning engine).
**Accept:** Reasoning chains resolve entity names during traversal. ✅

### TM-3.0-006 — Causal tracing integration into retrieval pipeline
**Status:** DONE
**What:** `CausalTrace` is now threaded through `RetrievalEngine::query()`. Every vector match, graph hop, and episodic trace adds attribution. `RetrievalResult` includes `causal_trace` field. MCP `memory_query` returns `explanation`. Tauri `QueryView` shows "Why these results?" collapsible panel.
**Accept:** Every query produces causal attribution data. ✅

### TM-3.0-007 — MCP reasoning tools
**Status:** DONE
**What:** Added `memory_reason`, `memory_analogies`, `memory_consolidate` to MCP JSON-RPC server. `memory_reason` supports both directed (source→target) and exploratory modes. All tools dispatch to tm-reason crate.
**Accept:** MCP clients can call all 3 reasoning tools. ✅

---

## Phase 3.1 — MIA-Inspired Retrieval Intelligence

### TM-3.1-001 — Composite retrieval scoring (MIA)
**Status:** DONE
**What:** Score(m) = 0.7*Sim + 0.15*Value + 0.15*Frequency. Value = successes/(usage+1), Frequency = 1/(usage+1). Added `retrieval_feedback` table to GraphStore with `record_retrieval()`, `record_success()`, `batch_value_scores()`, `batch_frequency_scores()`. Results re-ranked by composite score after vector search.
**Accept:** Entities that historically led to good outcomes rank higher. ✅

### TM-3.1-002 — Session context blending (MIA)
**Status:** DONE
**What:** Sim = 0.8*sim(query, memory) + 0.2*sim(session_context, memory). `blend_with_context()` computes centroid of recent query embeddings and blends 80/20 with current query. Prevents tunnel vision on exact query match.
**Accept:** Ambiguous queries get better results when session context exists. ✅

### TM-3.1-003 — Fallback cascade (MIA reflection)
**Status:** DONE
**What:** When max similarity < 0.3 and results are sparse, automatically try next bandit arm's parameters (wider top_k). Made `UcbBandit::params_for_arm()` public. Prevents returning poor results without trying harder.
**Accept:** Weak queries trigger automatic retry with wider strategy. ✅

### TM-3.1-004 — Entity success/failure credit assignment
**Status:** DONE
**What:** `explicit_feedback()` credits entities via `graph.record_success()` when reward > 0.5. Each `query()` calls `record_retrieval()` for usage tracking. Builds value scores over time.
**Accept:** Entities accumulate success/usage stats for composite scoring. ✅

### TM-3.1-005 — Contrastive trajectory storage (MIA)
**Status:** DONE
**What:** Added `TrajectoryOutcome` enum (Unknown/Success/Failure) and `query_class` to Trajectory type. `TrajectoryStore::tag_outcome()` tags trajectories. `consolidate_contrastive()` keeps shortest success + one failure per query class, preserves untagged. MIA principle: learn from both positive and negative exemplars.
**Accept:** `cargo test --workspace` passes. Contrastive consolidation prunes redundant trajectories. ✅

---

## Phase 3.2 — R1-Inspired Architecture Upgrades

### TM-3.2-001 — Memory-R1 CRUD operations
**Status:** DONE
**What:** Added `MemoryOp` enum (Add/Update/Noop/Delete) to `tm-types`. `IngestPipeline::decide_memory_op()` uses vector similarity to decide: >0.90 same type = Noop (duplicate), >0.90 diff type = Update, 0.75-0.90 = Update (merge + average embeddings), <0.75 = Add. IngestResult now includes `memory_ops` field tracking all decisions. Noop lightly reinforces (+0.02), Update merges embeddings and reinforces (+0.1).
**Accept:** `cargo test --workspace` passes. Graph doesn't grow unboundedly with duplicates. ✅

### TM-3.2-002 — Reciprocal Rank Aggregation (Graph-R1)
**Status:** DONE
**What:** Replaced MIA's fixed linear weights (0.7/0.15/0.15) with parameter-free Reciprocal Rank Aggregation. `rra_fuse()` computes `RRA(id) = Σ 1/(k + rank + 1)` over 3 ranked lists (similarity, value, frequency). k=60 smoothing constant. Immune to score scale differences — no weight tuning needed. 2 new tests.
**Accept:** RRA fusion ranks entities correctly. No arbitrary weights to tune. ✅

### TM-3.2-003 — KG-R1 schema-agnostic graph actions
**Status:** DONE
**What:** Added `GraphAction` enum (GetOutgoingPredicates, GetIncomingPredicates, FollowPredicate, ReverseFollow) to `tm-graph`. These 4 actions are provably sufficient to traverse any reasoning path in a directed KG (KG-R1, arXiv:2509.26383). `GraphAction::execute()` dispatches to GraphStore methods. Foundation for future learned traversal policies.
**Accept:** All 4 actions implemented and callable. ✅

### TM-3.2-004 — QueryPlanner (prefrontal cortex)
**Status:** DONE
**What:** New `QueryPlanner` in `tm-controller` with 6 plan actions: DirectLookup, BanditRetrieval, ReasoningChain, AnalogySearch, Decompose, Consolidate. Assesses query complexity (Simple/Moderate/Complex/Compound) from linguistic features. Extracts entity hints from capitalized words. `replan()` escalates strategy when results are poor (Graph-R1 "rethink" step). 8 tests.
**Accept:** Planner classifies queries correctly. Relationship queries → ReasoningChain, analogy → AnalogySearch, etc. ✅

### TM-3.2-005 — Planner-driven retrieval pipeline
**Status:** DONE
**What:** `RetrievalEngine` now runs planner as Phase 0 ("think" step) before retrieval. DirectLookup forces narrow arm (speed), ReasoningChain forces hybrid arm (graph hops), others let bandit decide. MCP `memory_query` auto-enriches responses: relationship queries get reasoning chains, analogy queries get structural matches — zero extra user interaction needed.
**Accept:** Queries auto-route to the right pipeline. MCP responses include plan metadata. ✅

---

## What's Next — Priority Roadmap

### Phase 3.3 — Contextual Bandit + Progressive Attenuation (HIGH IMPACT)

#### TM-3.3-001 — LinUCB contextual bandit (enhanced)
**Status:** DONE
**What:** `LinUcbBandit` with diagonal approximation + 3 enhancements from expert feedback:
1. **Exploration decay** — α anneals from 0.5 → 0.05 via multiplicative decay (0.995/pull). Prevents "still trying dumb strategies" after preferences stabilize.
2. **Arm feature sharing** — arms encoded as [breadth, depth] features on a latent axis. Reward signal for arm 2 (wide) strengthens arm 3 (deep) via Gaussian similarity kernel. Faster convergence with same data.
3. **Trajectory hint** — `select_with_hint(context, Some(arm))` accepts nearest-neighbor prior from trajectory store. +0.1 bonus for arm that succeeded on similar past query.

Retrieval engine uses `TrajectoryStore::nearest_successful_arm()` (cosine sim > 0.7) to provide hints automatically. 9 LinUCB tests (24 total in tm-controller).
**Accept:** Alpha decays, arm bias shares across similar arms, trajectory hints work. ✅

#### TM-3.3-002 — Progressive fallback attenuation (GraphRAG-R1)
**Status:** DONE
**What:** Replace binary fallback gate (sim < 0.3) with attenuated cascade. Each cascade step multiplied by decay factor 0.6. Prevents over-retrieval while still catching bad arms. Track cascade depth in trajectory for analysis. Attenuated score must pass 0.15 threshold to be included.
**Accept:** Fallback cascade applies 0.6× decay per step. `cargo test --workspace` passes. ✅

#### TM-3.3-003 — Simplified reward signal
**Status:** DONE
**What:** Replace composite reward with outcome-only signals: click=0.7 (positive engagement), rapid_requery=0.1 (results were bad), dwell>10s=0.5 (probably useful), else=0.3 (ambiguous). R1 papers consistently show simpler outcome-aligned rewards outperform complex proxies.
**Accept:** Reward signal is 4 clean cases. `cargo test --workspace` passes. ✅

### Phase 3.4 — Planner Decomposition + Procedural Execution

#### TM-3.4-001 — Query decomposition execution
**Status:** DONE
**What:** When planner returns `Decompose { sub_queries }`, `query_decomposed()` executes each sub-query independently with medium arm + 1-hop graph expansion, merges entities/triples with deduplication, and builds unified CausalTrace. Phase 0.5 intercept in `query()` routes Decompose plans automatically.
**Accept:** Compound queries split and merge correctly. `cargo test --workspace` passes. ✅

#### TM-3.4-002 — Procedural memory execution in query flow
**Status:** DONE
**What:** `ProcedureStore` wired into `RetrievalEngine`. `match_procedures()` scores active procedures by name substring match (1.0), name-word Jaccard, and step-text Jaccard (0.5×). Top 3 procedures with score > 0.2 surfaced in `RetrievalResult.procedures`. Auto-opened from `procedures.jsonl`.
**Accept:** Queries matching stored procedures surface them alongside entities. ✅

#### TM-3.4-003 — Uncertainty-driven routing
**Status:** DONE
**What:** `low_confidence` flag set when: (plan.confidence < 0.5 AND entities < 2) OR entities empty. `generate_suggestions()` produces 2-3 follow-up queries: "Tell me more about X", "How does X relate to Y?", "What's similar to X?". MCP response includes `confidence: { low, suggestions }`.
**Accept:** Low-confidence queries produce actionable suggestions. ✅

#### TM-3.4-004 — Diversity penalty (MMR reranking)
**Status:** DONE
**What:** Phase 2.9 in retrieval pipeline: Maximal Marginal Relevance greedy selection. After RRA ranking, penalize each entity by `λ * max_similarity_to_already_selected` (λ=0.3). Prevents returning 3 near-identical entities. Uses entity embeddings from GraphStore.
**Accept:** Builds and tests pass. Result lists have better coverage. ✅

#### TM-3.4-005 — Trajectory nearest-neighbor prior
**Status:** DONE
**What:** `TrajectoryStore::nearest_successful_arm()` finds most similar past successful trajectory (cosine sim > 0.7) and returns the arm that worked. Passed as hint to `LinUcbBandit::select_with_hint()`. Non-parametric prior — no learning, just lookup. "Similar past queries preferred arm X, so bias toward arm X."
**Accept:** Trajectory lookup + hint integration builds and passes. ✅

### Phase 3.5 — Paper-Inspired Architecture (BIGMAS + MEM)

#### TM-3.5-001 — QueryWorkspace (GWT shared state)
**Status:** DONE
**What:** `QueryWorkspace` struct with 4 GWT-inspired partitions (ctx/work/sys/ans). All 13 pipeline phases read/write to the workspace instead of passing state sequentially. Downstream phases can condition on upstream decisions. Inspired by BIGMAS (arXiv:2603.15371) centralized shared workspace from Global Workspace Theory.
**Accept:** Build passes, 3 retrieval tests pass. ✅

#### TM-3.5-002 — PhaseRecord execution history
**Status:** DONE
**What:** `PhaseRecord` struct logged by every pipeline phase: phase name, duration (μs), candidates in/out, human-readable decision string. Exposed via `RetrievalResult.phases`. Mirrors BIGMAS execution history ℋ^(t) — enables observability and smarter downstream decisions.
**Accept:** Build passes, phases populated in query results. ✅

#### TM-3.5-003 — Self-correction with error context
**Status:** DONE
**What:** `QueryPlanner::replan_with_context(plan, error_ctx)` uses failure description to select smarter retry strategy (BIGMAS self-correction loop). "0 entities" → escalate DirectLookup→BanditRetrieval→ReasoningChain. "Low similarity" → try AnalogySearch or Decompose. "Cascade exhausted" → confidence → 0.0. 4 tests.
**Accept:** `cargo test -p tm-controller` — 28 tests pass. ✅

#### TM-3.5-004 — Temporal decay weighting
**Status:** DONE
**What:** Added recency as 4th RRA fusion signal (alongside sim, value, frequency). Uses `batch_recency_scores()` — recently-updated entities rank higher. Inspired by MEM's temporal attention layers where recent observations get more weight.
**Accept:** Build passes, RRA now fuses 4 lists. ✅

#### TM-3.5-005 — Selective ingestion gate
**Status:** DONE
**What:** `IngestPipeline::should_ingest()` rejects noise before entity extraction (MEM "model decides what to remember"). Criteria: < 3 non-stopword tokens → skip, cosine sim > 0.95 to existing memory → skip. Skipped inputs logged to trace with reason. `IngestResult.skip_gate` flag. 3 tests.
**Accept:** `cargo test -p tm-ingest` — 13 tests pass. ✅

### Phase 4.0 — Production Polish

#### TM-4.0-001 — ONNX embeddings + model selection + quality benchmark
**Status:** DONE
**What:** 6 Apache 2.0 models via fastembed (BGE, BGE-Q, MiniLM, MiniLM-Q, Arctic, Arctic-Q — all 384-dim). Default changed to BGE-small-en-v1.5. `TM_EMBED_MODEL` env var for runtime model selection. `tm-bench` crate with embedding quality (15 triples), retrieval quality (24 docs, 8 queries), and `--compare` mode for side-by-side model evaluation. Hash embedder only for tests. BGE: 100% accuracy, 0.317 margin. MiniLM: 100% accuracy, 0.385 margin. Hash baseline: 60% accuracy, 0.040 margin.
**Accept:** `cargo run -p tm-bench` passes. `cargo run -p tm-bench -- --compare` shows all 6 models. ✅

#### TM-4.0-002 — Tauri desktop app packaging
**Status:** TODO
**What:** `cargo tauri build` for macOS .dmg. Menu bar integration. Auto-start option. System tray icon.
**Effort:** 2-3 days

#### TM-4.0-003 — Capture daemon integration
**Status:** TODO
**What:** Wire tm-capture into Tauri app. Background thread monitors clipboard/screen. Relevance gating via `is_relevant_for_ingestion()`. User notification on auto-ingest.
**Effort:** 2 days

#### TM-4.0-004 — Performance benchmarking
**Status:** TODO
**What:** Measure: ingest latency, query latency, memory usage (target <200MB idle, <500MB active), SQLite file size growth rate. Optimize hot paths.
**Effort:** 1 day
