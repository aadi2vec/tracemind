# TraceMind Architecture

**~12,000 lines of Rust | 14 crates | 116 tests | 4 binaries**

Local-only memory OS. All data lives in `~/.tracemind/`. No cloud, no telemetry.

---

## How to Read This Codebase

Start from the bottom and work up. TraceMind has 4 layers:

```
Layer 4: Interfaces        tm-cli, tm-mcp, tm-tauri, tm-capture
Layer 3: Intelligence      tm-retrieval, tm-reason, tm-controller
Layer 2: Storage           tm-graph, tm-vector, tm-episodic, tm-governance
Layer 1: Foundation        tm-types
```

### Recommended reading order

1. **`tm-types/src/`** (~940 lines) — Read all 8 files. Every struct in the system lives here: `Entity`, `Triple`, `Trace`, `Trajectory`, `Procedure`, `MemoryOp`, `TimeRange`. Zero I/O, pure data.

2. **`tm-graph/src/store.rs`** (~1,590 lines) — The heart. SQLite-backed knowledge graph with vector search, PageRank, Louvain communities, decay, KG-R1 graph actions, and temporal range queries all in one file.

3. **`tm-ingest/src/pipeline.rs`** (~780 lines) — Follow the data path: text → governance check → heuristic NER → dedup → Memory-R1 CRUD decision → graph upsert → triple extraction.

4. **`tm-controller/src/planner.rs`** (~610 lines) — The brain's prefrontal cortex. Classifies queries into 7 actions (including temporal), selects strategy, supports re-planning.

5. **`tm-controller/src/bandit.rs`** (~750 lines) — UCB1 + LinUCB contextual bandit. 5 arms (narrow/medium/wide/deep/colbert), learns which retrieval strategy works best per query type.

6. **`tm-retrieval/src/engine.rs`** (~1,670 lines) — The main query pipeline. Planner → bandit → vector search → RRA fusion → graph expansion → causal trace. Includes dedicated temporal query and decomposed query execution paths.

7. **`tm-reason/src/`** (~1,100 lines) — 4 modules: `chain.rs` (Graph-of-Thought), `causal.rs` (attribution), `analogy.rs` (WL kernel), `consolidation.rs` (Ebbinghaus decay).

8. **`tm-mcp/src/main.rs`** (~610 lines) — JSON-RPC server exposing 7 MCP tools. Good to read last — it's the integration layer.

---

## Brain-Inspired Architecture

TraceMind maps to a neuroscience-inspired agent model:

```
                         Query
                           │
            ┌──────────────▼──────────────┐
            │     PREFRONTAL CORTEX       │
            │     QueryPlanner            │
            │                             │
            │  • Assess complexity        │
            │  • Classify intent (7 types)│
            │  • Detect temporal intent   │
            │  • Select strategy          │
            │  • Re-plan on failure       │
            └──────────────┬──────────────┘
                           │
            ┌──────────────▼──────────────┐
            │     HIPPOCAMPUS             │
            │     RetrievalEngine         │
            │                             │
            │  • Session context blend    │
            │  • Multi-arm bandit (5 arms)│
            │  • Temporal retrieval path  │
            │  • ColBERT MaxSim (arm 4)   │
            │  • RRA rank fusion          │
            │  • Causal attribution       │
            └──────────────┬──────────────┘
                           │
            ┌──────────────▼──────────────┐
            │     REASONING CORTEX        │
            │     tm-reason               │
            │                             │
            │  • Graph-of-Thought chains  │
            │  • Analogical reasoning     │
            │  • Consolidation (sleep)    │
            └──────────────┬──────────────┘
                           │
            ┌──────────────▼──────────────┐
            │     MEMORY MANAGER          │
            │     IngestPipeline          │
            │                             │
            │  • CRUD ops (Add/Update/    │
            │    Noop/Delete)             │
            │  • Embedding merge          │
            │  • Contrastive trajectories │
            └─────────────────────────────┘
```

---

## Crate Map

### tm-types (Foundation — read first)
```
src/
├── entity.rs       Entity, EntityType (Person/Org/Project/Technology/Concept/Event/Location)
├── triple.rs       Triple, Predicate (12 types: RelatedTo, IsA, PartOf, DependsOn, ...)
├── trace.rs        Trace (immutable audit record)
├── trajectory.rs   Trajectory, TrajectoryOutcome (RL training data)
├── procedure.rs    Procedure, ProcedureStep (learnable actions)
├── memory_op.rs    MemoryOp (Add/Update/Noop/Delete — Memory-R1 inspired)
├── time_range.rs   TimeRange, parse_time_expression(), has_temporal_intent(), TEMPORAL_KEYWORDS
└── error.rs        TraceMindError
```

### tm-graph (Storage — 1,590 lines, 11 tests)
Single SQLite file holds everything:
- **Entities** with UUID, name, type, confidence, timestamps
- **Triples** with subject→predicate→object relationships
- **Vectors** (384-dim embeddings stored as BLOBs)
- **Access logs** for recency/novelty scoring
- **Retrieval feedback** table (usage count, success count per entity)
- **ColBERT tokens** table for per-token embeddings (late interaction)

Key APIs:
```rust
GraphStore::open("path")                        // Open/create database
graph.upsert_entity(&entity)                    // Insert or update
graph.search_vectors(&emb, top_k)               // Cosine similarity search
graph.k_hop_neighbors(id, hops)                 // BFS expansion
graph.pagerank()                                // Graph centrality
graph.louvain()                                 // Community detection
graph.outgoing_predicates(id)                   // KG-R1 action 1
graph.follow_predicate(id, pred)                // KG-R1 action 3
graph.batch_value_scores(&ids)                  // MIA success rate
graph.batch_frequency_scores(&ids)              // MIA exploration bonus
graph.get_entities_by_time_range(start, end)    // Temporal: entities updated in range
graph.get_accessed_entities_in_range(start, end)// Temporal: entities accessed in range
```

### tm-controller (Planning — ~1,360 lines, 32 tests)

**QueryPlanner** classifies queries into 7 actions:
| Action | Example | Routing |
|--------|---------|---------|
| TemporalQuery | "What was I working on last week?" | Temporal pipeline (time-range filter) |
| DirectLookup | "What is Rust?" | Force narrow arm |
| BanditRetrieval | "Tell me about ML frameworks" | LinUCB decides |
| ReasoningChain | "How does Rust relate to Python?" | Force hybrid arm + auto-chain |
| AnalogySearch | "What's similar to Rust?" | Auto-analogy |
| Decompose | "Compare Rust and Python and Go" | Split into sub-queries |
| Consolidate | "Clean up my memories" | Run consolidation |

**LinUcbBandit** — contextual bandit (diagonal approximation):
- Uses 384-dim query embedding as context
- Learns per-query-type arm preferences ("ML queries → wide arm, lookups → narrow arm")
- O(d) storage per arm (~3KB) vs O(d²) for full LinUCB (~1.2MB)
- Dual updates: both LinUCB and legacy UCB1 get reward signals
- Persists to `linucb.json` alongside `bandit.json`
- Backward-compatible serialization: old 4-arm JSON files auto-extend to 5

5 retrieval arms:
| Arm | Name | top_k | Hops | Episodic | ColBERT |
|-----|------|-------|------|----------|---------|
| 0 | narrow | 5 | 0 | no | no |
| 1 | medium | 10 | 1 | no | no |
| 2 | wide | 15 | 2 | no | no |
| 3 | deep | 20 | 2 | yes | no |
| 4 | colbert | 10 | 1 | no | yes |

### tm-retrieval (Pipeline — ~1,670 lines, 3 tests)

**QueryWorkspace** (GWT-inspired): All phases read/write a shared workspace struct with 4 partitions:
- `ctx` — read-only query context (text, embeddings, plan)
- `work` — read-write candidates, entities, triples, causal trace
- `sys` — execution metadata: arm, cascade depth, `Vec<PhaseRecord>`
- `ans` — final answer assembly (confidence, suggestions, procedures)

**PhaseRecord** execution history: Every phase logs what it did (duration, candidates in/out, decision string). Downstream phases can condition on upstream decisions. Persisted in `RetrievalResult.phases`.

**3 execution paths** based on planner action:
1. **Standard pipeline** (13 phases) — most queries
2. **Temporal pipeline** — "what was I working on last week?" (time-range filter)
3. **Decomposed pipeline** — "compare X and Y" (split, execute, merge)

The standard query pipeline has 14 phases:
```
Phase 0:    Planner assesses query → selects strategy (think step)
Phase 0.5a: Temporal intercept → time-range filtered retrieval
Phase 0.5b: Decompose intercept → split compound queries, merge results
Phase 1:    Embed query (needed for LinUCB context)
Phase 1.5:  LinUCB selects arm with trajectory hint (or planner overrides)
Phase 2:    Session context blend (80/20)
Phase 2.5:  Optional ColBERT reranking
Phase 2.6:  ColBERT MaxSim (arm 4 only — per-token late interaction)
Phase 2.7:  RRA fusion — 4-list rank aggregation (sim + value + freq + recency)
Phase 2.8:  Progressive fallback cascade — 0.6× attenuation, self-correction with error context
Phase 2.9:  MMR diversity penalty — greedy reranking (λ=0.3) for coverage
Phase 3:    K-hop graph expansion
Phase 4:    Triple collection + dedup
Phase 5:    Episodic traces + causal attribution + auto-enrich (chains/analogies)
Phase 6:    Procedural memory matching + confidence assessment + suggestions
```

The temporal pipeline has 6 phases:
```
temporal_entities:   SQLite json_extract on updated_at timestamps
temporal_access:     Access log entries in time range
temporal_traces:     JSONL traces with created_at in range
temporal_merge_rank: Dedup + rank by 60% vector sim + 40% recency
triples:             Collect relationships for result entities
confidence:          Low-confidence flag + suggestions
```

### tm-ingest (Extraction — 870 lines, 13 tests)

**Selective ingestion gate** (MEM-inspired): Before entity extraction, rejects noise:
- Too short / all stopwords (< 3 semantic tokens) → skip
- Near-exact duplicate (cosine sim > 0.95) → skip
- Skipped inputs logged to trace with reason

Memory-R1 CRUD decision at ingest time:
```
sim > 0.90 + same type  → Noop  (duplicate, reinforce +0.02)
sim > 0.90 + diff type  → Update (reclassify, reinforce +0.10)
sim 0.75–0.90           → Update (merge embeddings)
sim < 0.75 or no match  → Add   (novel entity)
```

### tm-reason (Intelligence — 1,100 lines, 9 tests)
- **ChainBuilder** — BFS multi-hop paths, hop decay 0.85^n
- **CausalTrace** — "why did I retrieve this?" attribution
- **AnalogySolver** — WL kernel fingerprint similarity
- **Consolidator** — Ebbinghaus: R(t) = e^(-t/S), S = 1 + accesses × 0.5

### tm-mcp (MCP Server — 650 lines)
7 tools over JSON-RPC 2.0 on stdin/stdout (auto-enriches temporal queries with time range metadata):
- `memory_store` — ingest text
- `memory_query` — retrieve + auto-route (chains/analogies)
- `get_trace` — audit trail
- `list_procedures` — stub
- `memory_reason` — explicit reasoning chains
- `memory_analogies` — structural similarity
- `memory_consolidate` — memory maintenance

---

## Data Flow

### Ingest
```
text → GovernanceFilter (PII regex) → heuristic NER → dedup (case-insensitive)
     → decide_memory_op (Memory-R1 CRUD) → GraphStore.upsert() → embed + store vector
     → extract_triples (pattern + co-occurrence) → TraceStore.append()
```

### Query (standard)
```
text → QueryPlanner.plan() → LinUCB.select() → embed + session blend
     → search_vectors() → ColBERT rerank → ColBERT MaxSim (arm 4)
     → RRA fusion (sim + value + freq + recency rankings)
     → k-hop graph expand → collect triples → optional episodic scan
     → CausalTrace (full attribution) → auto-enrich (chains/analogies)
     → deferred reward → TraceStore.append()
```

### Query (temporal)
```
text → QueryPlanner.plan() → detect temporal intent → parse_time_expression()
     → get_entities_by_time_range() + get_accessed_entities_in_range()
     → traces_in_range() → merge + dedup → rank by 60% sim + 40% recency
     → collect triples → CausalTrace → TraceStore.append()
```

### Feedback Loop
```
user feedback → UcbBandit.register_reward() → graph.record_success()
             → value scores improve → future RRA rankings improve
             → contrastive trajectory storage (shortest success + random failure)
```

---

## Research Papers Implemented

| Paper | What We Took | Where It Lives |
|-------|-------------|----------------|
| **MIA** (arXiv:2604.04503) | Composite scoring, session context blend, contrastive trajectories | tm-retrieval, tm-episodic |
| **Memory-R1** (arXiv:2508.19828) | ADD/UPDATE/NOOP/DELETE at ingest time | tm-ingest, tm-types |
| **Graph-R1** (arXiv:2507.21892) | Reciprocal Rank Aggregation (parameter-free fusion) | tm-retrieval |
| **KG-R1** (arXiv:2509.26383) | 4-action schema-agnostic graph API | tm-graph |
| **GraphRAG-R1** (arXiv:2507.23581) | Progressive fallback attenuation | tm-retrieval |
| **BIGMAS** (arXiv:2603.15371) | QueryWorkspace (GWT shared state), PhaseRecord execution history, self-correction with error context | tm-retrieval, tm-controller |
| **Pi MEM** (Physical Intelligence) | Selective ingestion gate, temporal decay weighting in RRA fusion | tm-ingest, tm-retrieval |

---

## File Inventory

| File | Lines | Purpose |
|------|-------|---------|
| `tm-retrieval/src/engine.rs` | 1,673 | Multi-phase retrieval pipeline (standard + temporal + decomposed) |
| `tm-graph/src/store.rs` | 1,589 | Knowledge graph + vectors + scoring + temporal range queries |
| `tm-tauri/src/main.rs` | 1,283 | Desktop app IPC (20 commands) |
| `tm-types/src/*.rs` | 940 | Domain types (8 files incl. TimeRange) |
| `tm-ingest/src/pipeline.rs` | 777 | Entity extraction + CRUD |
| `tm-controller/src/bandit.rs` | 747 | UCB1 + LinUCB contextual bandit (5 arms) |
| `tm-mcp/src/main.rs` | 651 | MCP server |
| `tm-controller/src/planner.rs` | 612 | Query planning (7 actions) |
| `tm-episodic/src/*.rs` | 560 | Trace/trajectory/procedure stores |
| `tm-rerank/src/lib.rs` | 450 | ColBERT reranker |
| `tm-cli/src/main.rs` | 378 | CLI binary |
| `tm-reason/src/chain.rs` | 350 | Graph-of-Thought reasoning |
| `tm-capture/src/main.rs` | 324 | Screen capture |
| `tm-reason/src/consolidation.rs` | 310 | Memory consolidation |
| `tm-reason/src/analogy.rs` | 250 | WL kernel analogies |
| `tm-governance/src/lib.rs` | 230 | PII filtering |
| `tm-reason/src/causal.rs` | 180 | Causal attribution |
| `tm-vector/src/embed.rs` | 120 | Embeddings |

---

## Data Files

All in `$TM_DATA_DIR` (default `~/.tracemind/`):

| File | Format | Size | Purpose |
|------|--------|------|---------|
| `memory.db` | SQLite | Grows | Entities, triples, vectors, access logs, feedback |
| `traces.jsonl` | JSONL | Append-only | Immutable audit trail |
| `procedures.jsonl` | JSONL | Small | Learnable action sequences |
| `bandit.json` | JSON | ~200B | UCB1 arm statistics |

---

## Build & Test

```bash
cargo build --release          # Build all 14 crates
cargo test --workspace         # Run all 116 tests
cargo test -p tm-controller    # Test single crate (32 tests)

# Run CLI
./target/release/tracemind ingest "Rust is a systems language"
./target/release/tracemind query "What is Rust?"
./target/release/tracemind status

# Run MCP server
./target/release/tm-mcp

# Override data dir
TM_DATA_DIR=/tmp/test ./target/release/tracemind ingest "test"
```

---

## Roadmap

### Current State (Phase 3.x — complete)

R1-inspired intelligence layer is fully operational:
- **Memory-R1 CRUD** — dedup at ingest time (Add/Update/Noop/Delete)
- **Graph-R1 RRA** — parameter-free rank fusion replaces hand-tuned weights
- **KG-R1 actions** — 4-action schema-agnostic graph API
- **GraphRAG-R1 attenuation** — 0.6× decay per cascade step
- **LinUCB contextual bandit** — diagonal approx + α decay + arm feature sharing + trajectory hint
- **QueryPlanner** — 6-action prefrontal cortex with replan escalation
- **Query decomposition** — compound queries split, executed, merged
- **Simplified rewards** — outcome-only signals (click/dwell/requery)
- **MIA session blending** — 80/20 query-context mix
- **Contrastive trajectories** — shortest-success + random-failure per query class
- **MMR diversity penalty** — greedy reranking prevents redundant results
- **Trajectory nearest-neighbor** — non-parametric prior biases arm selection
- **Procedural memory** — procedures surface alongside entity results
- **Uncertainty routing** — low-confidence flag + suggested follow-up queries
- **QueryWorkspace (GWT)** — 4-partition shared state for all pipeline phases (BIGMAS-inspired)
- **PhaseRecord history** — every phase logs duration, candidates, decisions (BIGMAS ℋ)
- **Self-correction with error context** — replan uses failure reason, not blind escalation
- **Temporal decay weighting** — recency as 4th RRA signal (MEM temporal attention)
- **Selective ingestion gate** — rejects noise before entity extraction (MEM selective memory)
- **Unified reasoning narrative** — 3-layer Strategy/Process/Evidence explanation answering "WHY did TraceMind recommend this?"
- **ColBERT retrieval arm** — 5th bandit arm with MaxSim scoring, per-token late interaction, backward-compatible serialization
- **ColBERT token cache** — SQLite storage for per-token embeddings (foundation for multi-vector retrieval)
- **Temporal queries** — "what was I working on last week?" NLP time expression parser (today/yesterday/last N days/weeks/months/recently/N ago), dedicated temporal retrieval pipeline with time-range entity/trace filtering, MCP temporal metadata enrichment
- **File/directory bulk import** — `tracemind import <path>` with extension filtering and dry-run
- **MCP procedures live** — list_procedures returns real stored procedures from ProcedureStore

### Phase 4 — Procedures & Production

| Item | What | Impact |
|------|------|--------|
| Procedural memory in query flow | Wire ProcedureStore into retrieval — surface matching procedures alongside entity results | Medium — closes the loop on learned actions |
| Uncertainty-driven routing | When planner confidence < 0.5, surface "not confident" + suggested follow-ups | Medium — honest about knowledge gaps |
| ONNX embeddings everywhere | Ship real all-MiniLM-L6-v2 via fastembed, auto-download on first run | High — real semantic search |
| Tauri desktop packaging | macOS .dmg, menu bar, auto-start, system tray | High — consumer distribution |
| Performance benchmarking | Target: <200MB idle, <500MB active, <50ms query p95 | High — production readiness |

### Phase 5 — Predictive Intelligence

| Item | What | Impact |
|------|------|--------|
| Semantic memory summarization | Cluster related entities → create super-entities with merged descriptions (MEM abstract consolidation) | High — hierarchical memory |
| Execution graph per query | QueryPlanner outputs multi-step DAG with Fork/Merge, not single action (BIGMAS GraphDesigner) | High — parallel sub-plans |
| JEPA encoder | Joint Embedding Predictive Architecture — predict next entity state from partial observation | High — anticipatory memory |
| World model | Internal simulation: "if I store X, what queries will it help?" | High — proactive memory management |
| Surprise-based ingestion | Only ingest if information gain exceeds threshold (KL divergence from world model) | Medium — prevents memory bloat |
| SSM/Mamba history compression | Replace linear session context with state-space model for long conversation history | Medium — better context over long sessions |

### Phase 6 — Production & Multi-User

| Item | What | Impact |
|------|------|--------|
| Tauri desktop packaging | macOS .dmg, menu bar, auto-start, system tray | High — consumer distribution |
| Capture daemon | Background clipboard/screen monitoring with relevance gating | Medium — passive memory collection |
| Docker Compose deployment | Containerized server mode for team use | Medium — enterprise path |
| Multi-user ACL | Per-user memory namespaces with access control | Medium — shared deployment |
| REST API | HTTP endpoints alongside MCP for broader integration | Low — MCP is primary interface |
| Performance benchmarking | Target: <200MB idle, <500MB active, <50ms query p95 | High — production readiness |
