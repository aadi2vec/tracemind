# TraceMind

**Local-only memory OS for humans and AI agents.**

TraceMind gives every context window a persistent, structured, privacy-first memory — running entirely on your machine. No cloud, no RAM bloat, no telemetry.

---

## What it does

When you (or a Claude Code session) ingest text, TraceMind:

1. **Filters** it through a PII/governance gate (email, phone, SSN, credit card — stdlib regex, no external deps)
2. **Extracts** entities (Person, Org, File, URL, Technology, Concept…) and typed relationships via multi-word-aware heuristic NER
3. **Embeds** entities with context-aware ONNX embeddings (`"entity_name: source_text"` → 384-dim MiniLM-L6-v2)
4. **Stores** entities, triples, and vectors in a **single SQLite file** (via [sqlite-knowledge-graph](https://crates.io/crates/sqlite-knowledge-graph))
5. **Logs** a provenance trace to an append-only JSONL file

When you query:

1. A **UCB1 bandit** picks one of 4 retrieval strategies (narrow / medium / wide / deep)
2. **Phase 1** — vector search for seed entities (cosine similarity)
3. **Phase 1.5** — optional ColBERT reranking (MaxSim late interaction, ~20-50ms on CPU)
4. **Phase 2** — k-hop graph traversal from seeds
5. **Phase 3** — episodic trace scan (if arm = deep)
6. The bandit learns from the result and adjusts future arm selection

---

## Workspace layout

```
crates/
├── tm-types        # All domain types: Entity, Triple, Trace, Trajectory, Procedure
├── tm-graph        # sqlite-knowledge-graph backed store (graph + vectors + algorithms)
├── tm-vector       # 384-dim ONNX embedder (all-MiniLM-L6-v2 via fastembed)
├── tm-episodic     # Append-only JSONL: traces, trajectories, procedures
├── tm-governance   # PII filter + confidence gate (stdlib only, no regex crate)
├── tm-controller   # UCB1 bandit: 4 retrieval arms, register_reward, arm_stats
├── tm-ingest       # Multi-word NER + typed predicates → graph + vector upsert
├── tm-rerank       # Optional ColBERT late-interaction reranker (feature-gated)
├── tm-retrieval    # 3-phase recall engine, bandit-guided, optional ColBERT reranking
├── tm-mcp          # MCP server: JSON-RPC 2.0 over stdin/stdout for Claude Code
├── tm-cli          # `tracemind` CLI binary
├── tm-tauri        # Tauri v2 desktop app (React + Tailwind)
└── tm-capture      # Background daemon: clipboard + shell history monitoring
```

---

## Quick start

```bash
# Build
cargo build --release

# Ingest something
tracemind ingest "Alice is the lead engineer on the TraceMind Rust project at Acme Corp"

# Query
tracemind query "who is working on TraceMind"

# Pipe from stdin
echo "Aaditya uses Rust for TraceMind" | tracemind ingest -

# See recent traces with audit detail
tracemind trace --limit 5
tracemind trace --id <prefix>   # drill into a specific trace

# UCB bandit statistics
tracemind status

# Decay confidence (forgetting curve)
tracemind decay --factor 0.95 --threshold 0.05

# Procedural memory
tracemind proc add "deploy" --desc "Deploy to prod" "git pull;cargo build --release;systemctl restart"
tracemind proc list
tracemind proc run deploy
tracemind proc feedback deploy --success
```

Data is stored under `~/.tracemind/` by default. Override with `TM_DATA_DIR`.

---

## Desktop app (Tauri)

```bash
# Install frontend deps
cd crates/tm-tauri/ui && bun install && cd ../../..

# Dev mode
cargo tauri dev

# Release build
cargo tauri build
```

Five views: **Dashboard** (stats + bandit visualization + live capture feed + recommendations), **Query** (semantic search + inline recommendations), **Ingest** (text extraction preview), **Graph** (force-directed knowledge graph with type filtering), **Traces** (audit trail with drill-down).

---

## Capture daemon

```bash
# Start background monitoring
tracemind-capture

# Environment variables
TM_DATA_DIR=~/.tracemind        # Data directory
TM_CLIP_INTERVAL_MS=1000        # Clipboard poll interval
TM_HIST_INTERVAL_S=5            # Shell history poll interval
TM_HASH_EMBED=0                 # Set to 1 for hash embeddings (no model)
```

Monitors clipboard (macOS `pbpaste`) and shell history (`~/.zsh_history`). Auto-ingests into TraceMind memory with deduplication and password detection.

---

## MCP integration (Claude Code)

Add to your Claude Code MCP config (`~/.claude/mcp.json` or workspace `.mcp.json`):

```json
{
  "mcpServers": {
    "tracemind": {
      "command": "/path/to/tm-mcp",
      "env": {
        "TM_DATA_DIR": "/Users/you/.tracemind"
      }
    }
  }
}
```

The MCP server exposes 4 tools:

| Tool | Description |
|---|---|
| `memory_store` | Ingest text into TraceMind memory |
| `memory_query` | Query memory with natural language |
| `get_trace` | Retrieve recent provenance traces |
| `list_procedures` | List stored procedural memories |

---

## Architecture

### Single-file storage

Everything lives in one SQLite file (`memory.db`) powered by [sqlite-knowledge-graph](https://crates.io/crates/sqlite-knowledge-graph):

| What | SQLite table | Notes |
|---|---|---|
| Entities | `kg_entities` | Typed nodes with JSON properties (UUID, confidence, timestamps) |
| Relations | `kg_relations` | Typed edges with weight (confidence) and properties |
| Vectors | `kg_vectors` | 384-dim float32 embeddings, brute-force cosine search |
| Hyperedges | `kg_hyperedges` | Higher-order relations (future use) |

Plus built-in graph algorithms: **PageRank**, **Louvain** community detection, **BFS/DFS** traversal, connected components.

### Cognitive pipeline

| Layer | Role | Storage |
|---|---|---|
| **Semantic** | Recall anything similar via embeddings | `kg_vectors` (SQLite) |
| **Structured** | Connect facts via typed relationships | `kg_entities` + `kg_relations` |
| **Episodic** | Record every ingest, query, and outcome | `traces.jsonl` |
| **Procedural** | Versioned how-to sequences with lifecycle | `procedures.jsonl` |

### NER pipeline

Multi-word aware, 2-pass extraction:
- **Pass 1:** Consecutive Title Case spans → multi-word entities (e.g. "Aaditya Srivathsan" → Person, "Acme Corp" → Organization)
- **Pass 2:** Single tokens → Technology (80+ known terms), Person, File, URL, Concept
- **150+ skip words** filter common verbs/adjectives from entity candidates
- **9 typed predicates:** WorksAt, uses, IsA, DependsOn, Produces, Owns, CollaboratesWith, PartOf, References
- **Context-aware embeddings:** `"entity_name: full_source_text"` captures semantic context

### UCB1 bandit arms

| Arm | Name | top_k | hops | episodic |
|---|---|---|---|---|
| 0 | vector-only | 5 | 0 | no |
| 1 | graph-heavy | 10 | 1 | no |
| 2 | hybrid | 15 | 2 | no |
| 3 | episodic | 20 | 2 | yes |

Selection formula: `arm* = argmax_a [ Q(a) + √(2 ln(N) / n(a)) ]`

---

## Design tenets

1. **Local-only.** No API calls, no telemetry. Your memory never leaves your machine.
2. **Auditability.** Every ingest and query produces a `Trace` with full provenance. Every decision is replayable.
3. **No RAM bloat.** Target: <200MB idle, <500MB active. SQLite + JSONL, no JVM, no Electron.
4. **Single-file storage.** One `.db` file contains your entire knowledge graph + vectors. Easy to backup, move, inspect.
5. **Graceful degradation.** The bandit starts with UCB1 (day 1). JEPA/World Model training (Phase 3) is opt-in and runs on CPU during idle.
6. **Structure controls learning.** The knowledge graph is never mutated by gradient descent. Only heuristics and human feedback write to it.

---

## Tech stack

| Component | Choice | Why |
|---|---|---|
| Language | Rust | Memory safety, zero-cost abstractions, tiny binaries |
| Graph + Vector store | sqlite-knowledge-graph (SQLite) | Single file, no daemon, graph algorithms built in |
| Embedder | fastembed (all-MiniLM-L6-v2 ONNX) | 384-dim, ~80MB, CPU inference, no Python |
| Reranker (opt) | mxbai-edge-colbert-v0-17m (ONNX) | 48-dim token embeddings, ~35MB, MaxSim scoring |
| Episodic store | Append-only JSONL | Simplest durable log; Parquet in Phase 3 |
| Desktop app | Tauri v2 + React + Tailwind | Native, <10MB overhead, no Electron |
| MCP server | JSON-RPC 2.0 over stdin/stdout | Claude Code native protocol |
| CLI | clap derive | Standard Rust CLI |

---

## Data schemas

### Entity
```rust
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: EntityType,   // Person | Org | Technology | File | Url | Concept | ...
    pub confidence: f64,           // [0.0, 1.0], decays over time
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### Triple
```rust
pub struct Triple {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate: Predicate,      // WorksAt | DependsOn | uses | IsA | RelatedTo | ...
    pub object_id: Uuid,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

---

## Test coverage

```
cargo test --workspace
```

```
tm-types         8 tests   Entity/Triple/Trajectory/Procedure lifecycle
tm-graph         8 tests   Entity CRUD, triple CRUD, k-hop BFS, decay, counts, vector search
tm-vector        3 tests   Embed length/norm, determinism, different texts differ
tm-episodic      7 tests   Trace recent(N), procedure lifecycle, trajectory filter
tm-governance    6 tests   Email/SSN/phone PII, confidence gate pass/fail
tm-controller    7 tests   UCB exploration, high-reward arm selection, clamping
tm-ingest        8 tests   URL/File/Concept entities, multi-word NER, typed predicates, PII rejection
tm-rerank        7 tests   MaxSim scoring, cosine similarity, ColBERT feature gate
tm-retrieval     1 test    Query → Ok, bandit pull recorded
─────────────────────────────
Total           55 tests   0 failures
```

---

## Roadmap

### Phase 1 — MVP ✅
- [x] 12-crate Rust workspace
- [x] Heuristic NER ingest pipeline (multi-word, typed predicates)
- [x] sqlite-knowledge-graph unified store (entities + triples + vectors)
- [x] UCB1 bandit retrieval controller
- [x] Append-only trace + trajectory stores
- [x] PII governance filter
- [x] MCP server for Claude Code
- [x] `tracemind` CLI with rich output + stdin pipe
- [x] ONNX embeddings (all-MiniLM-L6-v2)
- [x] Procedural memory with lifecycle FSM

### Phase 2 — Desktop + Capture ✅
- [x] Tauri v2 desktop app (React + Tailwind, 4 views)
- [x] Capture daemon (clipboard + shell history)
- [x] Context-aware embeddings
- [x] Confidence decay
- [x] Persistent bandit state

### Phase 2.5 — Graph Upgrade + Reranker ✅
- [x] Rewrite tm-graph → sqlite-knowledge-graph
- [x] Consolidate vector search into single SQLite file
- [x] ColBERT reranker (mxbai-edge-colbert-v0-17m, 48-dim, feature-gated)
- [x] LanceDB dependency removed

### Phase 3 — Latent Intelligence
- [ ] JEPA encoder (~5M params, VICReg loss)
- [ ] World Model for retrieval outcome prediction
- [ ] Surprise-based ingestion (`||predicted - actual||`)
- [ ] SSM/Mamba temporal compression
- [ ] PageRank-weighted entity importance
- [ ] Louvain community clustering

### Phase 4 — Distribution
- [ ] `brew install tracemind`
- [ ] Notarized macOS .dmg
- [ ] Browser extension for passive capture
- [ ] Auto-update via Tauri updater

---

## Palantir Ontology alignment

| Palantir | TraceMind |
|---|---|
| Objects (Nouns) | `Entity` nodes |
| Links (Relationships) | `Triple` edges |
| Properties | `confidence`, `source_id`, timestamps |
| Actions / Verbs | `Procedure` + `ProcedureStep` |
| Security | `GovernanceFilter` (PII + confidence gate) |
| Audit Trail | `Trace` + JSONL episodic store |
| Object Versioning | `Procedure.version` + `parent_id` |

---

## ColBERT reranker

Optional high-quality retrieval upgrade. Model: `mxbai-edge-colbert-v0-17m` (16.8M params, ~35MB, BEIR 0.490, 48-dim token embeddings).

**Architecture:** Late interaction — per-token embeddings with MaxSim scoring. Cross-encoder quality at bi-encoder speed. ~20-50ms on M1 Mac CPU.

**How it works:**
1. Vector search retrieves 3x wider candidate set
2. ColBERT encodes query (`[CLS] [Q] <tokens> [SEP] [MASK]...`) and each candidate doc (`[CLS] [D] <tokens> [SEP] [PAD]...`)
3. MaxSim scoring: for each query token, max cosine sim vs all doc tokens, sum across query tokens
4. Combined score: `alpha * rerank + (1 - alpha) * initial_vector_score`
5. Top-k candidates passed to graph expansion phase

**Enable:** `cargo build --features colbert` — requires `model.onnx` + `tokenizer.json` from `mxbai-edge-colbert-v0-17m`. Graceful fallback when disabled.

---

## Binary sizes (release)

```
target/release/tracemind          2.6 MB
target/release/tm-mcp             3.1 MB
target/release/tracemind-capture  2.8 MB
```

---

*For agent handoff context, see [CONTEXT.md](CONTEXT.md).*
