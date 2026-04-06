# TraceMind — Agent Context Document

> **Purpose:** This file contains everything an AI agent needs to pick up work on TraceMind.
> Updated: 2026-04-06. Source of truth for architecture, conventions, roadmap, and known issues.

---

## 1. What is TraceMind?

TraceMind is a **local-only memory OS for humans and AI agents**. It gives every context window a persistent, structured, privacy-first memory — running entirely on the user's machine. No cloud, no RAM bloat, no telemetry.

**Founder:** Aaditya Srivathsan — SWE/founder, deep Rust + Python + ML background, privacy-first conviction.

**Target users:**
- Consumer power users who want their research/browsing to compound into personal knowledge
- Claude Code / AI agent users who want agents to learn codebase conventions
- ML engineers who need trajectory storage for RL agent training

---

## 2. Repository Structure

```
tracemind/                          # Rust workspace, 12 crates
├── Cargo.toml                      # Workspace root
├── README.md                       # Public-facing README
├── CONTEXT.md                      # THIS FILE — agent handoff context
├── PHASE2-TASKS.md                 # Phase 2 task board (mostly complete)
├── docs/
│   ├── 00-consolidated-vision.md   # Strategic vision document
│   ├── 01-product-requirements.md  # Product requirements & personas
│   └── 02-engineering-architecture.md # Deep technical architecture spec
│
├── crates/
│   ├── tm-types/        # All domain types: Entity, Triple, Trace, Trajectory, Procedure
│   ├── tm-graph/        # sqlite-knowledge-graph backed store (entities + triples + vectors + algorithms)
│   ├── tm-vector/       # Embedder only (fastembed ONNX, 384-dim all-MiniLM-L6-v2)
│   ├── tm-rerank/       # Optional ColBERT reranker (feature-gated, mxbai-edge-colbert-v0-17m)
│   ├── tm-episodic/     # Append-only JSONL: traces, trajectories, procedures
│   ├── tm-governance/   # PII filter + confidence gate (stdlib only, no regex crate)
│   ├── tm-controller/   # UCB1 bandit: 4 retrieval arms, persistent state
│   ├── tm-ingest/       # Heuristic NER → graph + vector upsert pipeline
│   ├── tm-retrieval/    # 3-phase recall engine, bandit-guided
│   ├── tm-mcp/          # MCP server: JSON-RPC 2.0 over stdin/stdout for Claude Code
│   ├── tm-cli/          # `tracemind` CLI binary
│   ├── tm-tauri/        # Tauri v2 desktop app (React + Tailwind frontend)
│   └── tm-capture/      # Background daemon: clipboard + shell history monitoring
│
└── legacy/              # Python prototype (AgentMem v2), for reference only
```

---

## 3. Architecture

### 3.1 Cognitive Pipeline (5 layers)

| Layer | Role | Storage | Status |
|---|---|---|---|
| **Semantic** | Recall anything similar via embeddings | SQLite vector table (skg) | ✅ Done |
| **Structured** | Connect facts via typed relationships | SQLite entity/triple tables (skg) | ✅ Done |
| **Episodic** | Record every ingest, query, and outcome | Append-only JSONL | ✅ Done |
| **Procedural** | Store versioned how-to sequences | JSONL with lifecycle FSM | ✅ Done |
| **Clustered** | HDBSCAN semantic grouping | Not started | 🔮 Phase 3 |

### 3.2 Storage Architecture (Post-Rewrite)

**Before:** 2 backends — SQLite (graph via raw rusqlite) + LanceDB directory (vectors).
**After (current):** Single SQLite file via `sqlite-knowledge-graph` crate.

```
~/.tracemind/
├── memory.db         # sqlite-knowledge-graph: entities, relations, vectors, all in one
├── traces.jsonl      # Append-only trace log
├── procedures.jsonl  # Procedural memory
└── bandit.json       # UCB1 bandit persistent state
```

The `memory.db` file contains:
- `kg_entities` — Entity nodes with JSON properties (UUID, confidence, entity_type, timestamps)
- `kg_relations` — Typed edges (triples) with weights and JSON properties
- `kg_vectors` — 384-dim float32 vectors per entity, cosine similarity search
- `kg_hyperedges` — Higher-order relations (available, not yet used)
- Built-in graph algorithms: PageRank, Louvain community detection, BFS/DFS traversal

### 3.3 Key Type Mappings (TraceMind ↔ sqlite-knowledge-graph)

| TraceMind | skg | Notes |
|---|---|---|
| `Entity.id: Uuid` | `entity.properties["uuid"]` | UUID stored as JSON property, i64 is internal skg ID |
| `Entity.entity_type: EntityType` | `entity.entity_type: String` | Stored as JSON-serialized enum string |
| `Entity.confidence: f64` | `entity.properties["confidence"]` | In JSON properties |
| `Triple` | `Relation` | predicate → rel_type, confidence → weight |
| `Triple.id: Uuid` | `relation.properties["uuid"]` | Same pattern as entities |
| `GraphStore.search_vectors()` | `KnowledgeGraph.search_vectors()` | Brute-force cosine, fine for <100K |

### 3.4 Ingest Pipeline

```
Raw text → Governance check (PII filter) → Content hash
         → Entity extraction (multi-word NER, 2-pass)
         → Entity upsert (graph + vector embedding)
         → Triple extraction (pattern-based + co-occurrence)
         → Triple upsert → Trace record
```

**Entity extraction:**
- Pass 1: Consecutive Title Case spans → multi-word entities ("Aaditya Srivathsan", "Acme Corp")
- Pass 2: Single tokens → classify as Technology (KNOWN_TECH list), Person (title case), File, URL, Concept
- 150+ SKIP_WORDS filter removes common verbs/adjectives from entity candidates
- KNOWN_TECH: 80+ entries (Rust, Python, Docker, Kubernetes, etc.)
- ORG_SUFFIXES: Inc, Corp, LLC, Ltd, etc.

**Triple extraction:**
- 9 pattern types: WorksAt, uses, IsA, DependsOn, Produces, Owns, CollaboratesWith, PartOf, References
- Pattern matching: keyword position in text → find closest entity before/after keyword
- Co-occurrence fallback: entity pairs within window get RelatedTo at 0.4 confidence
- Typed predicates get 0.75 confidence

**Context-aware embeddings:** Each entity is embedded as `"entity_name: full_source_text"` so the vector captures semantic context.

### 3.5 Retrieval Pipeline

```
Query text → Embed → Vector search (top_k candidates)
           → Graph expansion (k-hop BFS from seeds)
           → Triple collection + sorting (typed first, then by confidence)
           → Episodic trace scan (if arm=deep)
           → Bandit reward registration → Trace record
```

**UCB1 Bandit Arms:**

| Arm | Name | top_k | hops | episodic |
|---|---|---|---|---|
| 0 | vector-only | 5 | 0 | no |
| 1 | graph-heavy | 10 | 1 | no |
| 2 | hybrid | 15 | 2 | no |
| 3 | episodic | 20 | 2 | yes |

Selection: `arm* = argmax_a [ Q(a) + √(2 ln(N) / n(a)) ]`

### 3.6 Desktop App (Tauri v2)

- **Backend:** Rust IPC commands bridging to `IngestPipeline`, `RetrievalEngine`, `TraceStore`
- **Frontend:** React + Vite + Tailwind, dark theme (bg: #0a0a0f, accent: #7c5cfc)
- **Views:** Dashboard (stats + bandit visualization + activity feed), Query, Ingest, Traces
- **Config:** `crates/tm-tauri/tauri.conf.json`, window 1200x800, min 900x600

### 3.7 Capture Daemon

`tracemind-capture` runs in the background:
- **Clipboard monitor:** Polls `pbpaste` every 1s, dedup by content hash, skips passwords (high entropy detection)
- **Shell history monitor:** Watches `~/.zsh_history` or `~/.bash_history`, prefixes commands with "shell command:"
- Config via env: `TM_DATA_DIR`, `TM_CLIP_INTERVAL_MS`, `TM_HIST_INTERVAL_S`, `TM_HASH_EMBED`

### 3.8 MCP Server

JSON-RPC 2.0 over stdin/stdout. 4 tools: `memory_store`, `memory_query`, `get_trace`, `list_procedures`.

---

## 4. Build & Test

```bash
# Prerequisites
source ~/.cargo/env
export PROTOC=/tmp/protoc/bin/protoc  # protobuf compiler for lance/arrow deps

# Build all
cargo build --release

# Run all tests (52 tests across 12 crates)
cargo test --workspace

# Run specific crate tests
cargo test -p tm-graph
cargo test -p tm-ingest

# CLI usage
cargo run -p tm-cli -- ingest "Alice works at Anthropic on AI safety"
cargo run -p tm-cli -- query "who works at Anthropic"
cargo run -p tm-cli -- status
cargo run -p tm-cli -- trace --limit 5

# Stdin pipe
echo "Some text to ingest" | cargo run -p tm-cli -- ingest -

# Tauri dev (requires bun)
export PATH="$HOME/.bun/bin:$PATH"
cd crates/tm-tauri/ui && bun install && cd ../../..
cargo tauri dev

# Capture daemon
cargo run -p tm-capture
```

### Binary names
- `tracemind` — CLI (tm-cli)
- `tm-mcp` — MCP server
- `tracemind-capture` — Background daemon

---

## 5. Key Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `sqlite-knowledge-graph` | 0.11.0 | Core graph + vector + algorithms (bundled SQLite) |
| `rusqlite` | 0.32 | Raw SQL access via skg's Connection |
| `fastembed` | 5 | ONNX embeddings (all-MiniLM-L6-v2, 384-dim) |
| `tokenizers` | 0.21 | BPE tokenizer for ColBERT (optional, colbert feature) |
| `ndarray` | 0.16 | Tensor manipulation for ColBERT (optional, colbert feature) |
| `tauri` | 2 | Desktop app framework |
| `seahash` | 4 | Content hashing for dedup |
| `clap` | 4 | CLI argument parsing |
| `uuid` | 1 | Entity/triple IDs |
| `chrono` | 0.4 | Timestamps |
| `tokio` | 1 | Async runtime (capture daemon, LanceDB) |

---

## 6. Test Coverage (55 tests, all passing)

```
tm-types        8 tests   Entity/Triple lifecycle, decay, confidence clamping
tm-graph        8 tests   Entity CRUD, triple CRUD, k-hop BFS, decay, counts, vector search
tm-vector       3 tests   Embed length/norm, determinism, different texts differ
tm-episodic     7 tests   Trace recent(N), procedure lifecycle, trajectory training filter
tm-governance   6 tests   Email/SSN/phone PII detection, confidence gate
tm-controller   7 tests   UCB exploration, high-reward arm selection, reward clamping
tm-ingest       8 tests   URL/File/Concept entities, PII rejection, multi-word NER, typed predicates
tm-rerank       7 tests   MaxSim scoring, cosine similarity, ColBERT feature gate
tm-retrieval    1 test    Query → Ok, bandit pull recorded
───────────────────────
Total          55 tests   0 failures, 0 warnings
```

---

## 7. Conventions & Patterns

### Code style
- Rust 2021 edition, workspace-level deps
- `tracing` for structured logging (not `println!` in library crates)
- `TraceMindError` enum in tm-types for all error types
- `Result<T>` = `std::result::Result<T, TraceMindError>` throughout
- Tests use `:memory:` SQLite or temp dirs, cleaned up after

### Entity type hierarchy
```rust
pub enum EntityType {
    Person, Organization, Project, File, Url,
    Concept, Technology, Decision, Event,
    Custom(String),
}
```

### Predicate types
```rust
pub enum Predicate {
    RelatedTo, IsA, PartOf, HasProperty,         // Generic
    WorksAt, CollaboratesWith, Owns,              // People/Org
    DependsOn, Produces, References,              // Projects/Files
    HasProcedure,                                  // Procedural
    Custom(String),                                // Extensible
}
```

### GraphStore API surface (what callers use)
```rust
impl GraphStore {
    pub fn open(path: &str) -> Result<Self>;
    pub fn upsert_entity(&self, entity: &Entity) -> Result<()>;
    pub fn get_entity(&self, id: Uuid) -> Result<Entity>;
    pub fn find_entity_by_name(&self, name: &str) -> Result<Option<Entity>>;
    pub fn upsert_triple(&self, triple: &Triple) -> Result<()>;
    pub fn get_triples_for_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>>;
    pub fn k_hop_neighbors(&self, entity_id: Uuid, hops: u32) -> Result<Vec<Entity>>;
    pub fn entity_count(&self) -> Result<usize>;
    pub fn triple_count(&self) -> Result<usize>;
    pub fn decay_all(&self, factor: f64, threshold: f64) -> Result<usize>;
    // New: vector search (replaces LanceDB)
    pub fn upsert_vector(&self, entity_id: Uuid, embedding: &[f32]) -> Result<()>;
    pub fn search_vectors(&self, query: &[f32], top_k: usize) -> Result<Vec<(Uuid, f32)>>;
    // New: graph algorithms
    pub fn pagerank(&self) -> Result<HashMap<Uuid, f64>>;
    pub fn inner(&self) -> &KnowledgeGraph;  // raw skg access
}
```

---

## 8. Current State & What Just Happened

### Completed work (2026-04-06, session 2 — brave-herschel):
1. **Removed LanceDB** entirely — dropped `VectorStore`, `search.rs`, all arrow/lancedb deps
2. **tm-vector** is now embedder-only: `Embedder` (fastembed ONNX) + hash fallback
3. **ColBERT reranker** (`tm-rerank`) — full ONNX inference pipeline with `mxbai-edge-colbert-v0-17m`:
   - BPE tokenization via `tokenizers` crate, query/doc markers, MASK padding, skiplist filtering
   - MaxSim scoring, L2-normalized 48-dim per-token embeddings
   - Feature-gated (`--features colbert`), graceful fallback when disabled
4. **Integrated reranker** into `tm-retrieval`: optional `with_reranker()`, 3x wider candidate pool, score interpolation
5. **Updated README.md** with new architecture, test counts, tech stack
6. All 55 tests passing (62 including doc-tests), clean build

### Completed work (2026-04-06, session 1 — crazy-spence):
1. **Rewrote tm-graph** from raw `rusqlite` to `sqlite-knowledge-graph` crate
2. **Consolidated vector search** into GraphStore (replaces LanceDB for ingest/retrieval)
3. **Updated tm-ingest** to use `graph.upsert_vector()` instead of `VectorStore.upsert()`
4. **Updated tm-retrieval** to use `graph.search_vectors()` instead of `VectorStore.search()`

### Previously completed (earlier sessions):
- Full ingest quality improvements (context-aware embeddings, multi-word NER, typed predicates, skip-word filters)
- Observability: trace audit trails, raw text storage, rich CLI output, stdin pipe support
- Tauri desktop app scaffold + React frontend (4 views)
- Capture daemon (clipboard + shell history)
- MCP server with 4 tools
- All committed at `eca0dad`, pushed to `origin/claude/crazy-spence`

---

## 9. Roadmap & Future Work

### Phase 2.5 — Complete ✅
- [x] Rewrite tm-graph → sqlite-knowledge-graph
- [x] Consolidate vector search into single SQLite file
- [x] ColBERT reranker (mxbai-edge-colbert-v0-17m, feature-gated, integrated into retrieval)
- [x] Remove LanceDB dependency entirely (tm-vector is now embedder-only)
- [x] Update README.md with new architecture

### Phase 3 — Latent Intelligence
- [ ] JEPA encoder (~5M params MLP, VICReg loss) for latent retrieval
- [ ] World Model (~2–5M params MLP) for retrieval outcome prediction
- [ ] Surprise-based ingestion: replace static confidence gate with `||predicted - actual||`
- [ ] SSM/Mamba (~1–3M params) temporal compression of episodic history
- [ ] All models train on CPU during idle, no GPU required
- [ ] PageRank-weighted entity importance (skg already provides this)
- [ ] Louvain community detection for topic clustering (skg provides this)

### Phase 4 — Distribution
- [ ] `brew install tracemind` / cargo-binstall
- [ ] Notarized macOS .dmg
- [ ] Auto-update via Tauri updater
- [ ] Browser extension for passive web capture

### Phase 5 — Enterprise
- [ ] Docker Compose deployment
- [ ] Multi-user governance (ACL, audit log)
- [ ] REST API layer
- [ ] Team knowledge sharing with differential privacy

### Quality Improvements Backlog
- [ ] Replace heuristic NER with a small on-device model (e.g., GLiNER)
- [ ] Sentence-level triple extraction (currently per-text)
- [ ] Entity merging/deduplication (fuzzy name matching + embedding similarity)
- [ ] Temporal decay scheduling (cron or background timer)
- [ ] Graph visualization in Tauri app (D3 force-directed, skg has export_json for D3)
- [ ] Import/export (backup to JSON, restore from JSON)

---

## 10. ColBERT Reranker Plan

**Status:** Researched, not yet implemented. Add as optional high-quality retrieval step.

**Model:** `mxbai-edge-colbert-v0-17m` — 16.8M params, ~35MB fp16, BEIR score 0.490 (beats MiniLM's ~0.42)

**Architecture:** Late interaction — each query token attends to each document token separately, then max-sim aggregation. ~20-50ms on M1 Mac CPU.

**Integration plan:**
1. Add `tm-colbert` crate (optional, feature-gated)
2. Use as reranker on top of existing vector search: `vector search (top 50) → ColBERT rerank → top 10`
3. Rust crates to evaluate: `pylate-rs` (Candle-based), `ort` (ONNX Runtime), `tessera-embeddings`
4. Store ColBERT token embeddings per entity for fast reranking
5. Feature flag: `--features colbert` to enable, graceful fallback if not available

**Why ColBERT over cross-encoder:**
- Cross-encoder: O(n) forward passes per query (slow for >10 candidates)
- ColBERT: Pre-compute document embeddings, only query embedding at inference time
- Late interaction gives cross-encoder-like quality at bi-encoder-like speed

---

## 11. Known Issues & Gotchas

1. **UUID mapping overhead:** GraphStore maintains in-memory `HashMap<Uuid, i64>` for UUID↔skg mapping. Rebuilt on open by scanning all entities. Fine for <100K entities, may need optimization for larger stores.

2. **Brute-force vector search:** skg's vector search does a full table scan with cosine similarity. Fine for <100K vectors. For larger stores, enable skg's TurboQuant index (`kg.build_turboquant_index()`).

3. **LanceDB removed:** `tm-vector` is now embedder-only (fastembed). All LanceDB deps have been removed from the workspace.

4. **Tauri dev path:** `beforeDevCommand` must use `cd crates/tm-tauri/ui && bun run dev` (not just `cd ui`) because cargo tauri dev runs from workspace root.

5. **Tauri icon:** Must be RGBA PNG (color_type=6), not RGB. 128x128px minimum.

6. **ONNX model download:** First run with real embeddings downloads ~80MB model. Use `--hash-embed` for offline/test mode.

7. **skg relation weight range:** `Relation::new()` validates weight ∈ [0.0, 1.0]. GraphStore clamps confidence before creating relations.

8. **skg depth limit:** `get_neighbors()` has a max depth of 5. Deeper traversals will error.

9. **No relation update in skg:** `sqlite-knowledge-graph` has no `update_relation()` method. GraphStore uses raw SQL for triple updates.

10. **Properties stored as JSON strings:** All TraceMind metadata (UUID, confidence, timestamps, source_id) is stored in skg's JSON properties column. Parsing overhead exists but is negligible for current scale.

---

## 12. Performance Targets

| Metric | Target | Current |
|---|---|---|
| Idle RAM | <200 MB | ~50 MB (CLI), ~120 MB (Tauri) |
| Active RAM | <500 MB | ~200 MB with ONNX model loaded |
| Install size | <250 MB | ~80 MB (CLI + model) |
| Ingest latency | <200 ms | ~50-100 ms (with ONNX) |
| Query latency (p95) | <100 ms | ~30-80 ms |
| Binary size (CLI) | <10 MB | 2.6 MB |
| Binary size (MCP) | <10 MB | 3.1 MB |

---

## 13. Critical Files Quick Reference

| What | Path |
|---|---|
| Workspace config | `Cargo.toml` |
| Graph store (main rewrite) | `crates/tm-graph/src/store.rs` |
| Ingest pipeline + NER | `crates/tm-ingest/src/pipeline.rs` |
| Retrieval engine | `crates/tm-retrieval/src/engine.rs` |
| CLI binary | `crates/tm-cli/src/main.rs` |
| Tauri app | `crates/tm-tauri/src/main.rs` |
| Tauri frontend | `crates/tm-tauri/ui/src/` |
| MCP server | `crates/tm-mcp/src/main.rs` |
| Capture daemon | `crates/tm-capture/src/main.rs` |
| Domain types | `crates/tm-types/src/lib.rs` |
| UCB1 bandit | `crates/tm-controller/src/bandit.rs` |
| Embedder (ONNX) | `crates/tm-vector/src/embed.rs` |
| ColBERT reranker | `crates/tm-rerank/src/reranker.rs` |
| PII governance | `crates/tm-governance/src/filter.rs` |
| Tauri config | `crates/tm-tauri/tauri.conf.json` |

---

## 14. Git & Branch Info

- **Main branch:** `main`
- **Working branch:** `claude/crazy-spence`
- **Last commit:** `eca0dad` (Phase 3: Tauri app + capture daemon + all quality improvements)
- **Uncommitted:** sqlite-knowledge-graph rewrite (this session)
- **Remote:** `origin` → `github.com/aadi2vec/tracemind`
