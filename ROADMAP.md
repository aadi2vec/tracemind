# TraceMind Roadmap — Consolidated Task Board

**Date:** 2026-04-15
**Status legend:** `DONE` | `TODO` | `IN_PROGRESS` | `BLOCKED`
**Sorted by:** Biggest impact first within each tier

---

## Completed Work (Phases 2.0 – 4.1)

All items below are DONE and shipped on `main`. Preserved for context — do not re-open.

<details>
<summary>36 completed tasks (click to expand)</summary>

| ID | Phase | Title |
|----|-------|-------|
| P2-001 | 2.0 | Graph store + tracing (rusqlite) |
| P2-002 | 2.0 | Upstream crate API fix (Embedder::new → Result) |
| P2-003 | 2.0 | End-to-end CLI smoke test |
| P2-004 | 2.0 | Real ONNX embeddings (fastembed, 384-dim) |
| P2-005 | 2.0 | Procedural memory executor (JSONL lifecycle) |
| P2-006 | 2.0 | Confidence decay (`decay_all`) |
| P2-007 | 2.0 | Persistent bandit across CLI commands |
| P2-008 | 2.0 | Tauri desktop app scaffold + IPC |
| P2-009 | 2.0 | Tauri UI: query + dashboard |
| P2-010 | 2.0 | Tauri UI: memory timeline + entity graph |
| 2.6-001 | 2.6 | Entity deduplication (case-insensitive) |
| 2.6-002 | 2.6 | Explicit feedback (thumbs up/down) |
| 2.6-003 | 2.6 | PageRank-weighted recommendations |
| 2.6-004 | 2.6 | Community coloring (Louvain) |
| 2.6-005 | 2.6 | "Surprising entities today" widget |
| 2.6-006 | 2.6 | Batch SQL scoring (N+1 fix) |
| 2.6-007 | 2.6 | Entity trend sparklines |
| 2.6-008 | 2.6 | Entity delete from graph |
| 3.0-001 | 3.0 | Graph-of-Thought reasoning chains |
| 3.0-002 | 3.0 | Causal tracing / attribution |
| 3.0-003 | 3.0 | Analogical reasoning (WL kernel) |
| 3.0-004 | 3.0 | Memory consolidation ("sleep") |
| 3.0-005 | 3.0 | `find_entity_by_id` |
| 3.0-006 | 3.0 | Causal tracing integration into retrieval |
| 3.0-007 | 3.0 | MCP reasoning tools |
| 3.1-001 | 3.1 | MIA composite retrieval scoring |
| 3.1-002 | 3.1 | Session context blending (80/20) |
| 3.1-003 | 3.1 | Fallback cascade (MIA reflection) |
| 3.1-004 | 3.1 | Entity success/failure credit assignment |
| 3.1-005 | 3.1 | Contrastive trajectory storage |
| 3.2-001 | 3.2 | Memory-R1 CRUD operations |
| 3.2-002 | 3.2 | Reciprocal Rank Aggregation (RRA) |
| 3.2-003 | 3.2 | KG-R1 schema-agnostic graph actions |
| 3.2-004 | 3.2 | QueryPlanner (prefrontal cortex) |
| 3.2-005 | 3.2 | Planner-driven retrieval pipeline |
| 3.3-001 | 3.3 | LinUCB contextual bandit (enhanced) |
| 3.3-002 | 3.3 | Progressive fallback attenuation |
| 3.3-003 | 3.3 | Simplified reward signal |
| 3.4-001 | 3.4 | Query decomposition execution |
| 3.4-002 | 3.4 | Procedural memory in query flow |
| 3.4-003 | 3.4 | Uncertainty-driven routing |
| 3.4-004 | 3.4 | Diversity penalty (MMR reranking) |
| 3.4-005 | 3.4 | Trajectory nearest-neighbor prior |
| 3.5-001 | 3.5 | QueryWorkspace (GWT shared state) |
| 3.5-002 | 3.5 | PhaseRecord execution history |
| 3.5-003 | 3.5 | Self-correction with error context |
| 3.5-004 | 3.5 | Temporal decay weighting |
| 3.5-005 | 3.5 | Selective ingestion gate (MEM) |
| 4.0-001 | 4.0 | ONNX embeddings + model selection + benchmark |
| 4.1-001 | 4.1 | Unified reasoning narrative |
| 4.1-002 | 4.1 | ColBERT token cache infrastructure |
| 4.1-003 | 4.1 | File/directory bulk import CLI |
| 4.1-004 | 4.1 | MCP list_procedures fix |
| 4.1-005 | 4.1 | ColBERT retrieval arm (5th bandit arm) |
| 4.1-006 | 4.1 | Temporal queries |

</details>

---

## TIER 1 — Highest Impact (Architecture-defining)

These are the moves that determine whether TraceMind is a toy or a product.

---

### TM-5.1-001 — Two-speed ingestion pipeline (embed-first, extract-later)
**Status:** TODO
**Impact:** CRITICAL — enables passive capture without NER bottleneck
**Effort:** 3-4 days
**Deps:** none
**Why:** Current pipeline forces every input through heuristic NER (3/10 quality) before storing. For the passive capture daemon (clipboard, browser, shell), this is both slow and lossy — "debugging OAuth refresh loop" extracts nothing useful. The embedding already captures the full semantic content.
**What:**
1. **Fast path (capture):** `embed(text)` → store to `captured_signals` with embedding BLOB + timestamp. No NER, no graph writes. Target: <5ms per capture.
2. **Slow path (consolidation):** Cluster recent signals by embedding similarity → concatenate cluster texts → run heuristic NER on richer combined context → promote to entities. Runs on idle or timer.
3. Extend `captured_signals` schema: add `embedding BLOB`, `cluster_id`, `promoted_entity_id`.
4. Recommendations from fast path: `cosine_sim(new_signal, existing_entities) > 0.6` → surface connection. Zero NER needed.
**Accept:** Capture daemon stores signals at <5ms latency. Consolidation pass clusters and promotes signals to entities. `cargo test --workspace` passes.
**Sources:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — artifacts paper proves embedding-indexed observations reduce memory requirements 4-16x without explicit extraction.

---

### TM-5.1-002 — MCP structured ingestion protocol
**Status:** TODO
**Impact:** CRITICAL — closes the NLP quality gap to Honcho-level when LLM is present, at zero binary cost
**Effort:** 1-2 days
**Deps:** none
**Why:** When Claude (or any LLM) calls `memory_store`, the LLM already understands the text but we throw away that understanding by accepting raw text and re-parsing with regex. The LLM IS the NER model — just give it the right schema.
**What:**
1. Extend `memory_store` inputSchema with optional structured fields:
   - `facts: string[]` — atomic self-contained observations extracted by the calling LLM
   - `entities: [{name, type}]` — pre-extracted entities
   - `relations: [{subject, predicate, object}]` — pre-extracted triples
   - `supersedes: string` — what prior knowledge this contradicts/updates
2. When structured fields present: skip heuristic NER, directly upsert provided entities/triples, store facts as first-class observations.
3. When only `text` provided: fall back to current heuristic pipeline (CLI / non-LLM path).
4. `supersedes` field triggers contradiction detection: find matching entity, mark as superseded, link new → old.
**Accept:** MCP tool schema updated. Structured ingest bypasses NER. Fallback to heuristic still works. `cargo test -p tm-mcp` passes.
**Sources:** [Honcho](https://github.com/plastic-labs/honcho) Deriver pattern — except the calling LLM is the deriver. Zero additional model weight. [@akshay_pachaar](https://x.com/akshay_pachaar/status/2043745099792953508) — "conflict resolution is essential."

---

### TM-5.1-003 — Observation hierarchy (explicit/deductive/inductive/contradiction)
**Status:** TODO
**Impact:** HIGH — transforms flat entity store into layered knowledge; prerequisite for dreaming
**Effort:** 3-4 days
**Deps:** TM-5.1-002 (structured ingestion provides the raw material)
**Why:** All entities are currently flat — no distinction between "user said X" (explicit), "X implies Y" (deduced), "user tends to Z" (induced), or "X supersedes Y" (contradiction). [Honcho](https://github.com/plastic-labs/honcho)'s 4-level observation hierarchy with provenance trees is their key differentiator and a primary reason they achieve SOTA on LoCoMo (89.9%) and LongMem S (90.4%).
**What:**
1. Add `level` field to `Entity`: `Explicit | Deductive | Inductive | Contradiction`.
2. Add `derived_from: Vec<Uuid>` provenance chain — which entities was this derived from.
3. Add `superseded_by: Option<Uuid>` — for contradiction tracking.
4. MCP `facts` array → stored as `Explicit` level entities.
5. Consolidation-created entities → stored as `Deductive` or `Inductive`.
6. Query-time: prioritize higher-level observations; surface contradictions explicitly.
**Accept:** Entity type includes level field. Provenance chain persists. Query results respect observation levels. `cargo test --workspace` passes.
**Sources:** [Honcho 3](https://blog.plasticlabs.ai/blog/Honcho-3) — explicit → deductive → inductive hierarchy. [Benchmarking Honcho](https://blog.plasticlabs.ai/research/Benchmarking-Honcho) — SOTA results attributed to layered observations.

---

### TM-5.1-004 — Offline dreaming (consolidation that creates new knowledge)
**Status:** TODO
**Impact:** HIGH — the leap from "memory store" to "memory that thinks"
**Effort:** 1 week
**Deps:** TM-5.1-001 (signal clusters), TM-5.1-003 (observation hierarchy)
**Why:** Current `Consolidator` only decays/prunes/merges. It never creates new understanding. [Honcho](https://github.com/plastic-labs/honcho)'s Dreamer runs deduction and induction specialists that generate new derived knowledge. [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) Theorem 1 proves artifacts (derived observations) reduce information-theoretic cost of representing history by 4-16x.
**What:**
1. **Signal clustering:** Group un-promoted `captured_signals` by embedding cosine similarity (>0.7) into temporal clusters.
2. **Deduction pass:** For each cluster, identify entity pairs that co-occur across multiple signals → create `Deductive` level entities/triples capturing the relationship.
3. **Induction pass:** Across all clusters from last N days, identify recurring patterns (entity X appears in >3 clusters) → create `Inductive` level entities capturing behavioral trends.
4. **Surprisal scoring:** Flag observations that don't fit existing clusters (cosine sim to all centroids < 0.4) as high-surprisal → prioritize for attention.
5. Run as background task on timer or Tauri idle detection.
**Accept:** Consolidation creates new Deductive/Inductive entities. Surprisal scores computed. Runs without blocking capture path.
**Sources:** [Honcho Dreamer](https://github.com/plastic-labs/honcho) — deduction/induction specialists. [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — artifact compression theorem.

---

### TM-5.1-005 — Contradiction detection + supersession tracking
**Status:** TODO
**Impact:** HIGH — prevents memory corruption over time; most-cited failure mode of naive memory systems
**Effort:** 2-3 days
**Deps:** TM-5.1-003 (observation hierarchy provides the `superseded_by` field)
**Why:** When a user ingests "I'm vegetarian" after "I love steak", current Memory-R1 CRUD sees 0.75-0.90 cosine similarity and merges the embeddings — destroying both facts. The system should detect the contradiction, mark the old fact as superseded, and store both with a link.
**What:**
1. **Embedding divergence heuristic:** When Memory-R1 decides `Update` (0.75-0.90 sim), additionally check if the texts have opposing sentiment/semantics (simple: check for negation words, antonym patterns, "switched from", "no longer", "stopped").
2. **MCP `supersedes` field:** When the calling LLM provides this, directly mark the matching entity as `superseded_by` the new one.
3. **Query-time:** Filter out superseded entities by default. Include them only when the query is explicitly temporal ("what did I used to think about X?").
4. **Audit trail:** Superseded entities are never deleted — only hidden from default retrieval.
**Accept:** Contradictions detected via both heuristic and MCP paths. Superseded entities hidden from queries. Temporal queries surface history. Tests pass.
**Sources:** [@akshay_pachaar](https://x.com/akshay_pachaar/status/2043745099792953508) — "A vector DB will retrieve both 'I love steak' and 'I'm now vegetarian' as equally relevant." [Honcho](https://github.com/plastic-labs/honcho) — contradiction as first-class observation level.

---

### TM-5.1-006 — Context budget allocator (token-aware MCP responses)
**Status:** TODO
**Impact:** HIGH — prevents context window waste; directly improves downstream LLM quality
**Effort:** 2-3 days
**Deps:** TM-5.1-003 (observation hierarchy for pruning priority)
**Why:** MCP `memory_query` currently returns ALL retrieved entities/triples/traces with no token budget. When Claude's context fills up, we're sending noise alongside signal. [Ramp Labs' Latent Briefing](https://x.com/RampLabs/status/2042660310851449223) shows 31-49% token waste in naive context passing. [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) proves artifact-containing histories can be compressed without information loss.
**What:**
1. Add optional `max_tokens` parameter to `memory_query` (default: 4000).
2. Estimate token count per entity/triple/trace (simple: `len(json) / 4`).
3. **Greedy fill by relevance:** Pack highest-scored results first until budget exhausted.
4. **Artifact pruning:** If entity A's `derived_from` chain includes entity B, and both are in results, drop B (the derived entity encodes B's information per Theorem 1).
5. **Level priority:** Inductive > Deductive > Explicit when budget is tight (higher-level = more information-dense).
6. Return `budget_used` and `budget_total` in response metadata.
**Accept:** MCP responses respect token budget. Artifact pruning reduces redundancy. Response includes budget metadata.
**Sources:** [Ramp Labs Latent Briefing](https://x.com/RampLabs/status/2042660310851449223) — 31% token savings. [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — Theorem 1 artifact compression.

---

## TIER 2 — High Impact (Product-defining)

These are what make TraceMind feel like a product, not a CLI tool.

---

### TM-5.2-001 — Tauri desktop app packaging
**Status:** TODO
**Impact:** HIGH — distribution vehicle for everything consumer-facing
**Effort:** 2-3 days
**Deps:** none (scaffold exists from P2-008/009/010)
**What:** `cargo tauri build` for macOS .dmg. Menu bar integration. Auto-start option. System tray icon. Code signing for macOS Gatekeeper.
**Accept:** `.dmg` installs and launches. System tray icon present. Auto-start toggle works.
*Previously: TM-4.0-002*

---

### TM-5.2-002 — Capture daemon integration into Tauri
**Status:** TODO
**Impact:** HIGH — passive monitoring is the killer UX
**Effort:** 2 days
**Deps:** TM-5.2-001 (Tauri app), TM-5.1-001 (two-speed pipeline)
**What:** Wire `tm-capture` into Tauri as background thread. Use the fast-path (embed-only) from TM-5.1-001. User notification on auto-capture with relevance reason. Settings panel: toggle clipboard/shell/browser monitoring, adjust polling intervals, view capture log. Cross-platform clipboard: `pbpaste` (macOS), `powershell Get-Clipboard` (Windows), `xclip` (Linux).
**Accept:** Tauri app captures clipboard changes in background. User sees notification. Settings panel works.
*Previously: TM-4.0-003*

---

### TM-5.2-003 — Natural language answers
**Status:** TODO
**Impact:** HIGH — users want answers, not database rows
**Effort:** 2-3 days
**Deps:** TM-5.1-003 (observation hierarchy for richer source material)
**Why:** `memory_query` returns raw entity lists. "What do I know about OAuth?" should return a synthesized paragraph, not `[{name: "OAuth", type: "technology"}]`.
**What:**
1. **Template-based synthesis:** For each query, assemble a natural language response from: entities (names + types), triples (subject-predicate-object sentences), reasoning narrative, temporal context.
2. **Response format:** `{"answer": "You've been working on OAuth token refresh in auth-service. It depends on...", "sources": [...], "entities": [...]}`.
3. For MCP: the `answer` field is what the LLM reads. `sources` provides attribution.
4. For CLI: pretty-print the answer.
**Accept:** `memory_query` returns natural language answer alongside raw data. Templates cover common query types.
*Previously: TM-5.0-001*

---

### TM-5.2-004 — Proactive surfacing ("TraceMind noticed...")
**Status:** TODO
**Impact:** HIGH — makes memory feel alive, not passive
**Effort:** 2-3 days
**Deps:** TM-5.1-001 (signal capture), TM-5.2-002 (Tauri for notifications)
**Why:** Users shouldn't have to query to get value. When new information connects to existing knowledge, surface it proactively.
**What:**
1. On ingest/capture, check if new entity/signal bridges two previously unconnected graph clusters (community detection via Louvain already exists).
2. On ingest, check if new signal is highly similar to a recent query (user is actively thinking about this topic).
3. Surface via: Tauri notification, MCP event, or CLI log.
4. Format: "TraceMind noticed: your clipboard about OAuth refresh is related to the auth-service debugging you did yesterday."
**Accept:** Proactive notifications fire on bridge connections and query-signal matches. User can dismiss/disable.
*Previously: TM-5.0-002*

---

### TM-5.2-005 — Standard memory benchmark evaluation (LoCoMo, LongMem S, BEAM)
**Status:** TODO
**Impact:** HIGH — can't claim competitive positioning without benchmarks
**Effort:** 3-5 days
**Deps:** TM-5.1-002 (structured ingestion for fair comparison)
**Why:** [Honcho publishes SOTA results](https://blog.plasticlabs.ai/research/Benchmarking-Honcho): LoCoMo 89.9%, LongMem S 90.4%. TraceMind has no results on standard benchmarks. Even losing reveals exactly where to invest. The benchmark harness itself is a durable asset.
**What:**
1. Implement LoCoMo evaluation: 300-turn dialogues across 35 sessions, test recall of facts, temporal ordering, and contradictions.
2. Implement LongMem S evaluation: long-term single-session memory.
3. Integrate with `tm-bench` as additional evaluation modes.
4. Report: accuracy, P@K, MRR, latency, memory usage.
5. Compare: TraceMind (heuristic NER) vs TraceMind (structured MCP) vs baseline (vector-only).
**Accept:** Benchmark harness runs. Results published. Gap analysis identifies top-3 weaknesses.
**Sources:** [Honcho evals](https://evals.honcho.dev/), [Benchmarking Honcho](https://blog.plasticlabs.ai/research/Benchmarking-Honcho).

---

### TM-5.2-006 — Iterative/agentic retrieval loop
**Status:** TODO
**Impact:** MEDIUM-HIGH — Honcho's single most important benchmark finding
**Effort:** 1 week
**Deps:** TM-5.1-003 (observation hierarchy)
**Why:** Current 14-phase pipeline is deterministic — same phases, same order, every time. [Honcho's key finding](https://blog.plasticlabs.ai/research/Benchmarking-Honcho): "agentic retrieval (the Dialectic Agent with tool-calling) was the single most important change for achieving SOTA." The retriever should reason about what to fetch next based on what it's already found.
**What:**
1. After initial retrieval, evaluate result quality (confidence score, entity count, similarity spread).
2. If quality < threshold: automatically try alternative strategies:
   - Switch bandit arm (already possible via fallback cascade)
   - Expand graph hops from top results
   - Decompose query differently
   - Search for contradictions/superseded facts
3. Cap iterations at 3 to bound latency.
4. Track iteration history in PhaseRecord for observability.
5. No LLM needed — this is a local reasoning loop using existing pipeline components.
**Accept:** Retrieval iterates when initial results are poor. Quality improves on ambiguous queries. Latency bounded.
**Sources:** [Honcho Dialectic Agent](https://blog.plasticlabs.ai/research/Benchmarking-Honcho) — agentic retrieval as the key SOTA driver.

---

### TM-5.2-007 — Global hotkey quick-capture (Cmd+Shift+M)
**Status:** BLOCKED
**Impact:** MEDIUM-HIGH — instant access changes daily usage patterns
**Effort:** 2-3 days
**Deps:** TM-5.2-001 (Tauri desktop app)
**What:** System-wide hotkey registration via Tauri global shortcut API. Floating overlay window for quick text capture and inline search results. Dismiss on Escape or focus loss. Two modes: store (type + Enter) and search (type + see results).
**Accept:** Cmd+Shift+M opens overlay. Can store and search. Dismisses cleanly.
*Previously: TM-4.0-005*

---

### TM-5.2-008 — Performance benchmarking + optimization
**Status:** TODO
**Impact:** MEDIUM — need baselines before scaling; validates <200MB idle target
**Effort:** 1-2 days
**Deps:** none
**What:**
1. Measure: ingest latency (heuristic vs structured), query latency (per arm), capture-path latency, memory usage (idle/active), SQLite file size growth rate.
2. Profile hot paths: embedding (expect ~5ms), vector search O(n) scan, graph BFS.
3. Identify when O(n) vector search becomes the bottleneck (expected: ~10k entities).
4. Document results in `BENCHMARKS.md`.
**Accept:** Benchmark suite runs. Results documented. Top-3 bottlenecks identified.
*Previously: TM-4.0-004*

---

### TM-5.2-009 — File import real embedding integration
**Status:** TODO
**Impact:** MEDIUM — import exists but needs proper embedding verification
**Effort:** Half day
**Deps:** none
**What:** Wire the `import` command through the real ONNX embedder. Integration test with large file sets. Verify embedding quality on code vs prose content. Ensure file imports use fast-path when available.
**Accept:** `tracemind import ./src` produces real embeddings. Quality verified on mixed content.
*Previously: TM-5.0-004*

---

### TM-5.2-010 — Obsidian vault integration
**Status:** TODO
**Impact:** HIGH — distribution vehicle with ~2M pre-educated users
**Effort:** 3-5 days (import) + 1 week (plugin)
**Deps:** TM-5.1-002 (structured ingestion for quality)
**Why:** Obsidian has ~2M users who already understand local-first knowledge management — the exact target demographic (privacy-conscious, willing to self-host, pre-educated on second-brain concepts). Complementary positioning: Obsidian is where humans deliberately write; TraceMind is where the system remembers everything else. Fastest path to 1000+ users without requiring Tauri packaging.
**What:**
1. **Phase A — Vault import (3 days):**
   - Extend `tracemind import` with `--obsidian` flag
   - Parse frontmatter YAML (tags, aliases, dates)
   - Resolve `[[wiki-links]]` into graph relations (source note → target note as `References`)
   - Respect `.obsidianignore` and `.trash/` exclusions
   - Watch mode: monitor vault directory for changes (notify-rs crate)
2. **Phase B — Bidirectional sync (2 days):**
   - Promoted entities from consolidation can optionally write back as `.md` notes in a configured vault subfolder
   - Preserve user edits — never overwrite human-authored notes
   - Two-way UUID mapping: note path ↔ entity ID stored in frontmatter
3. **Phase C — Obsidian plugin (1 week, separate repo):**
   - TypeScript plugin using Obsidian's API
   - Connects to local TraceMind MCP server (HTTP bridge or stdio via Node child process)
   - Commands: "Ask TraceMind", "Find related notes", "Show memory graph"
   - Sidebar view with proactive recommendations
   - Distribute via Obsidian community plugin store
**Accept:**
- Phase A: `tracemind import --obsidian ~/vault` imports notes with wiki-link relations. Watch mode detects new notes within 5s.
- Phase B: Consolidated entities appear as notes in vault, round-trip preserves data.
- Phase C: Plugin installable from Obsidian store, connects to local TraceMind, sidebar shows live recommendations.
**Sources:** Obsidian has no built-in AI memory — TraceMind fills the gap without replacing Obsidian's manual-note workflow.

---

## TIER 3 — Medium Impact (Competitive Moat)

These are what separate TraceMind from the pack long-term.

---

### TM-5.3-001 — Multi-agent memory sharing (observer/observed)
**Status:** TODO
**Impact:** MEDIUM — future-critical for multi-agent MCP workflows
**Effort:** 1 week
**Deps:** TM-5.1-003 (observation hierarchy)
**Why:** [Honcho](https://github.com/plastic-labs/honcho)'s peer paradigm models what Agent A knows about Agent B (directional knowledge). [Ramp Labs' Latent Briefing](https://x.com/RampLabs/status/2042660310851449223) solves efficient context sharing between agents. As MCP multi-agent workflows grow, TraceMind needs per-agent views of shared memory.
**What:**
1. Add `observer_id` and `observed_id` to observations/entities.
2. Scope queries by observer: "what does Agent A know about the user?"
3. Session isolation: each MCP session gets its own view.
4. Conflict resolution: when two agents update the same entity, keep both with provenance.
**Accept:** Multiple MCP sessions maintain separate views. Queries scoped by observer. Conflicts preserved.
**Sources:** [Honcho peer paradigm](https://github.com/plastic-labs/honcho), [Latent Briefing](https://x.com/RampLabs/status/2042660310851449223).

---

### TM-5.3-002 — Artifact-aware retrieval pruning
**Status:** TODO
**Impact:** MEDIUM — principled information-theoretic optimization
**Effort:** 2-3 days
**Deps:** TM-5.1-003 (observation hierarchy with provenance chains)
**Why:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) Theorem 1 proves: if observation A deterministically encodes observation B, then B can be dropped from history without losing mutual information with the future. TraceMind's traces and derived entities create exactly these artifact chains.
**What:**
1. During retrieval, if entity A has `derived_from` containing entity B, and both appear in results, drop B.
2. During consolidation, identify artifact chains (A derived from B derived from C) and mark terminal artifacts.
3. Context budget allocator (TM-5.1-006) uses artifact chains for principled pruning.
**Accept:** Retrieval results contain no redundant artifact chains. Token savings measurable.
**Sources:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — Theorem 1 (artifact reduction), Definition 3 (externalized memory).

---

### TM-5.3-003 — Artifactless-copy evaluation framework
**Status:** TODO
**Impact:** MEDIUM — rigorous A/B testing methodology for memory value
**Effort:** 2-3 days
**Deps:** TM-5.2-005 (standard benchmarks)
**Why:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) Definition 3 and Proposition 1 provide a principled A/B framework: create an "artifactless copy" (randomize retrieval results) and measure performance delta. This quantifies how much value TraceMind actually adds vs baseline.
**What:**
1. Add `--artifactless` flag to `tm-bench` that randomizes retrieval results (preserves count, randomizes content).
2. Run standard benchmarks in both modes.
3. The delta = externalized memory benefit.
4. Report as: "TraceMind provides X% externalized memory benefit on [benchmark]."
**Accept:** Artifactless mode implemented. Delta measurable. Integrated into benchmark suite.
**Sources:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — Definition 3 (externalized memory), Proposition 1.

---

### TM-5.3-004 — Capacity-aware bandit arm selection
**Status:** TODO
**Impact:** MEDIUM — smarter retrieval when memory store is sparse vs rich
**Effort:** 1-2 days
**Deps:** none
**Why:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) finds low-capacity agents benefit most from artifacts (structured external memory). TraceMind's bandit should prefer narrower arms when the store is rich with traces/entities (artifacts do the work), and wider arms when sparse.
**What:**
1. Add "store richness" features to LinUCB context vector: entity count (log-scaled), signal count, recent query density.
2. LinUCB learns to select narrower arms for rich stores, wider for sparse.
3. No hardcoded rules — the bandit learns the relationship.
**Accept:** LinUCB context includes store richness. Arm selection adapts to store density. Tests pass.
**Sources:** [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) — capacity-artifact tradeoff.

---

### TM-5.3-005 — Browser extension (Chrome/Firefox)
**Status:** TODO
**Impact:** MEDIUM — passive capture from browsing, biggest untapped signal source
**Effort:** 3-5 days
**Deps:** TM-5.2-001 (Tauri), TM-5.1-001 (fast-path capture)
**What:** Chrome/Firefox extension that captures highlighted text + page URL + title. Sends to TraceMind via local HTTP endpoint or native messaging. Uses fast-path (embed-only). Optional: capture full page text on bookmark.
**Accept:** Extension captures highlights. Signals appear in TraceMind. Cross-browser.
*Previously: TM-5.0-003*

---

### TM-5.3-006 — Optional local micro-NER model (GLiNER/NuNER)
**Status:** TODO
**Impact:** MEDIUM — closes NER quality gap for offline/non-LLM paths without full LLM weight
**Effort:** 3-5 days
**Deps:** TM-5.1-001 (two-speed pipeline to integrate as optional third tier)
**Why:** Tier 1 (heuristic NER) is 3/10 quality. Tier 2 (MCP structured) requires an LLM. For power users who want better offline extraction without an LLM, a 50M-param NER-specific ONNX model closes the gap at ~200MB additional weight (vs 1.5GB+ for a general LLM).
**What:**
1. Evaluate GLiNER-small (50M params, ~200MB ONNX) and NuNER v2 (30M params, ~120MB ONNX) for zero-shot NER quality.
2. Add as optional feature-gated dependency (like ColBERT): `--features micro-ner`.
3. Use for consolidation slow-path when no LLM is available.
4. Benchmark: heuristic vs micro-NER vs LLM-structured on same corpus.
**Accept:** Feature-gated micro-NER runs. Quality measurably better than heuristic. Binary size increase <200MB.

---

### TM-5.3-007 — Vector search indexing (HNSW)
**Status:** TODO
**Impact:** MEDIUM — required for scaling past 10k entities
**Effort:** 2-3 days
**Deps:** none
**Why:** Current vector search is O(n) full table scan in SQLite. Acceptable for <10k entities, but passive capture + browsing will accumulate 10k+ signals within weeks.
**What:**
1. Evaluate: SQLite FTS5 with vector extension, or build HNSW index in-process (usearch crate, ~500KB).
2. Maintain backward compatibility: existing SQLite files should auto-migrate.
3. Benchmark: query latency at 1k, 10k, 100k, 1M vectors.
**Accept:** Vector search sub-10ms at 100k vectors. Migration transparent.

---

## TIER 4 — Future (Phase 5+ Research)

These are research-grade items from the original Phase 5 roadmap.

---

### TM-6.0-001 — JEPA encoder + world model
**Status:** TODO (RESEARCH)
**Impact:** LONG-TERM — surprise-based ingestion, predictive memory
**Effort:** 2-4 weeks
**Deps:** TM-5.1-004 (dreaming infrastructure)
**What:** Joint Embedding Predictive Architecture for learning what to expect next. Surprise signal (prediction error) drives ingestion priority: unexpected observations get stored with high priority. Trajectory fields (`predicted_outcome`, `actual_outcome_embedding`, `surprise_score`) already stubbed in `tm-types`.

### TM-6.0-002 — SSM/Mamba history compression
**Status:** TODO (RESEARCH)
**Impact:** LONG-TERM — compress long trace histories into fixed-size state
**Effort:** 2-3 weeks
**Deps:** TM-6.0-001
**What:** State-space model for compressing JSONL trace history into a fixed-size latent state. Enables O(1) "what happened in the last month?" queries without scanning the full trace file.

### TM-6.0-003 — Multi-user ACL + REST API
**Status:** TODO
**Impact:** MEDIUM — required for team/enterprise deployment
**Effort:** 1-2 weeks
**Deps:** TM-5.2-001 (Tauri), TM-5.3-001 (multi-agent)
**What:** Docker Compose deployment. Per-user access control. REST API wrapper around MCP tools. Rate limiting. Audit logging.

---

## Execution Order (Recommended)

```
IMMEDIATE (next 2 weeks):
  TM-5.1-001 (two-speed pipeline)     ← unlocks passive capture
  TM-5.1-002 (MCP structured ingest)  ← unlocks LLM-quality extraction
  TM-5.2-009 (file import embeddings) ← half day, low-hanging fruit
  TM-5.2-008 (perf benchmarking)      ← establishes baselines

THEN (weeks 3-4):
  TM-5.1-003 (observation hierarchy)  ← depends on 5.1-002
  TM-5.1-005 (contradiction detection)← depends on 5.1-003
  TM-5.1-006 (context budget)         ← depends on 5.1-003
  TM-5.2-001 (Tauri packaging)        ← parallel track

THEN (weeks 5-6):
  TM-5.1-004 (offline dreaming)       ← depends on 5.1-001 + 5.1-003
  TM-5.2-002 (capture daemon in Tauri)← depends on 5.2-001 + 5.1-001
  TM-5.2-003 (NL answers)             ← depends on 5.1-003
  TM-5.2-004 (proactive surfacing)    ← depends on 5.1-001 + 5.2-002

THEN (weeks 7-8):
  TM-5.2-005 (standard benchmarks)    ← depends on 5.1-002
  TM-5.2-006 (iterative retrieval)    ← depends on 5.1-003
  TM-5.2-007 (global hotkey)          ← depends on 5.2-001
  TM-5.2-010 (Obsidian integration)   ← Phase A: vault import, depends on 5.1-002
  TM-5.3-004 (capacity-aware bandit)  ← independent

LATER:
  TM-5.3-001 (multi-agent)
  TM-5.3-002 (artifact pruning)
  TM-5.3-003 (artifactless eval)
  TM-5.3-005 (browser extension)
  TM-5.3-006 (micro-NER model)
  TM-5.3-007 (vector indexing)

RESEARCH:
  TM-6.0-001 (JEPA)
  TM-6.0-002 (SSM/Mamba)
  TM-6.0-003 (multi-user ACL)
```

---

## External Intelligence Sources

| Source | Key Insight for TraceMind | Referenced In |
|--------|---------------------------|---------------|
| [@akshay_pachaar](https://x.com/akshay_pachaar/status/2043745099792953508) | Vector search alone is insufficient; contradiction detection is essential | TM-5.1-002, TM-5.1-005 |
| [Ramp Labs Latent Briefing](https://x.com/RampLabs/status/2042660310851449223) | 31-49% token waste in naive context passing; task-adaptive compression | TM-5.1-006, TM-5.3-001 |
| [Honcho / Plastic Labs](https://github.com/plastic-labs/honcho) | Observation hierarchy + agentic retrieval = SOTA; peer paradigm for multi-agent | TM-5.1-003, TM-5.1-004, TM-5.2-005, TM-5.2-006, TM-5.3-001 |
| [arXiv 2604.08756](https://arxiv.org/abs/2604.08756) | Artifacts reduce memory cost 4-16x; artifactless-copy eval framework | TM-5.1-001, TM-5.1-006, TM-5.3-002, TM-5.3-003, TM-5.3-004 |
| [Cognee](https://github.com/topoteretes/cognee) | Graph-vector hybrid baseline; TraceMind already surpasses this | Context only |
| [Honcho evals](https://evals.honcho.dev/) | LoCoMo 89.9%, LongMem S 90.4% — target to beat | TM-5.2-005 |
