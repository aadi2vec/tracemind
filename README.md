# TraceMind

**Local-only memory OS for humans and AI agents.**

TraceMind gives every context window a persistent, structured, privacy-first memory — running entirely on your machine. No cloud, no RAM bloat, no telemetry.

---

## What it does

When you (or a Claude Code session) ingest text, TraceMind:

1. **Filters** it through a PII/governance gate (email, phone, SSN, credit card — stdlib regex, no external deps)
2. **Extracts** entities (Person, Org, File, URL, Concept…) and typed relationships via heuristic NER
3. **Embeds** entity names into a 384-dim vector space
4. **Stores** entities + triples in a SQLite entity graph and vectors in a SQLite vector store
5. **Logs** a provenance trace to an append-only JSONL file

When you query:

1. A **UCB1 bandit** picks one of 4 retrieval strategies (narrow / medium / wide / deep)
2. **Phase 1** — vector search for seed entities
3. **Phase 2** — k-hop graph traversal from seeds
4. **Phase 3** — episodic trace scan (if arm = deep)
5. The bandit learns from the result and adjusts future arm selection

---

## Workspace layout

```
crates/
├── tm-types        # All domain types: Entity, Triple, Trace, Trajectory, Procedure
├── tm-graph        # SQLite entity + triple store, k-hop BFS traversal
├── tm-vector       # 384-dim embedder + cosine search over SQLite
├── tm-episodic     # Append-only JSONL: traces and trajectories
├── tm-governance   # PII filter + confidence gate (stdlib only, no regex crate)
├── tm-controller   # UCB1 bandit: 4 retrieval arms, register_reward, arm_stats
├── tm-ingest       # Heuristic NER → graph + vector upsert pipeline
├── tm-retrieval    # 3-phase recall engine, bandit-guided
├── tm-mcp          # MCP server: JSON-RPC 2.0 over stdin/stdout for Claude Code
└── tm-cli          # `tracemind` CLI binary
```

---

## Quick start

```bash
# Build
cargo build --release

# Ingest something
tracemind ingest "Alice is the lead engineer on the TraceMind Rust project"

# Query
tracemind query "who is working on TraceMind"

# See recent traces
tracemind trace --limit 5

# UCB bandit statistics
tracemind status
```

Data is stored under `~/.tracemind/` by default. Override with `TM_DATA_DIR`.

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
| `list_procedures` | List stored procedural memories (Phase 2) |

---

## Architecture

### Cognitive pipeline

| Layer | Role | Storage |
|---|---|---|
| **Semantic** | Recall anything similar via embeddings | SQLite vector table |
| **Structured** | Connect facts via typed relationships | SQLite entity/triple tables |
| **Episodic** | Record every ingest, query, and outcome | Append-only JSONL |
| **Procedural** | Store versioned how-to sequences linked to entities | Graph + vector hybrid (Phase 2) |

### UCB1 bandit arms

| Arm | top_k | hops | episodic |
|---|---|---|---|
| 0 — narrow | 5 | 0 | no |
| 1 — medium | 10 | 1 | no |
| 2 — wide | 15 | 2 | no |
| 3 — deep | 20 | 2 | yes |

Selection formula: `arm* = argmax_a [ Q(a) + √(2 ln(N) / n(a)) ]`

### Controller FSM

```
Idle → Evaluating → VectorSearch → BanditSelect → [GraphHop?] → [EpisodicScan?]
     → ComputeReward → RegisterReward → LogTrace → Idle
```

---

## Design tenets

1. **Local-only.** No API calls, no telemetry. Your memory never leaves your machine.
2. **Auditability.** Every ingest and query produces a `Trace` with full provenance. Every decision is replayable.
3. **No RAM bloat.** Target: <200MB idle, <500MB active. SQLite + JSONL, no JVM, no Electron.
4. **Graceful degradation.** The bandit starts with UCB1 (day 1). JEPA/World Model training (Phase 3) is opt-in and runs on CPU during idle.
5. **Structure controls learning.** The knowledge graph is never mutated by gradient descent. Only heuristics and human feedback write to it.

---

## Tech stack

| Component | Choice | Why |
|---|---|---|
| Language | Rust | Memory safety, zero-cost abstractions, tiny binaries |
| Graph store | SQLite (rusqlite bundled) | No JVM, no daemon, embedded |
| Vector store | SQLite cosine scan | Brute-force is fine at <100K vectors; swap to LanceDB in Phase 2 |
| Embedder | Hash-based stub (384-dim) | Deterministic, no model download for MVP; swap to ONNX + all-MiniLM-L6-v2 in Phase 2 |
| Episodic store | Append-only JSONL | Simplest durable log; Parquet in Phase 3 |
| MCP server | JSON-RPC 2.0 over stdin/stdout | Claude Code native protocol |
| CLI | clap derive | Standard Rust CLI |

---

## Data schemas

### Entity
```rust
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: EntityType,   // Person | Org | Project | File | Url | Concept | ...
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
    pub predicate: Predicate,      // RelatedTo | WorksAt | DependsOn | HasProcedure | ...
    pub object_id: Uuid,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### Trajectory (JEPA/WM-ready from day 1)
```rust
pub struct Trajectory {
    pub arm_chosen: u8,
    pub reward: f64,
    pub context_embedding: Vec<f32>,         // 384-dim
    pub memory_snapshot_hash: String,
    pub predicted_outcome: Option<Vec<f32>>, // Phase 3: JEPA prediction
    pub actual_outcome_embedding: Option<Vec<f32>>,
    pub surprise_score: Option<f32>,         // Phase 3: ||predicted - actual||
    // ...
}
```

---

## Test coverage

```
cargo test --workspace
```

```
tm-types        8 tests   Entity/Triple/Trajectory/Procedure lifecycle
tm-graph        5 tests   Insert, upsert, name lookup, k-hop BFS (1-hop, 2-hop)
tm-vector       6 tests   Embed length/norm, determinism, cosine top-1, top_k cap
tm-episodic     2 tests   Trace recent(N), trajectory pending_training filter
tm-governance   6 tests   Email/SSN/phone PII, confidence gate pass/fail
tm-controller   7 tests   UCB exploration order, high-reward arm selection, clamping
tm-ingest       3 tests   URL entity, File entity, PII rejection
tm-retrieval    1 test    Query → Ok, bandit pull recorded
─────────────────────────────
Total          42 tests   0 failures
```

---

## Roadmap

### Phase 1 — MVP (current)
- [x] 10-crate Rust workspace
- [x] Heuristic NER ingest pipeline
- [x] SQLite graph + vector stores
- [x] UCB1 bandit retrieval controller
- [x] Append-only trace + trajectory stores
- [x] PII governance filter
- [x] MCP server for Claude Code
- [x] `tracemind` CLI

### Phase 2 — Real embeddings + Tauri app
- [ ] Swap hash embedder → ONNX + all-MiniLM-L6-v2 (80MB, CPU inference)
- [ ] Swap SQLite cosine scan → LanceDB (Rust-native, ANN index)
- [ ] Swap SQLite graph → Kuzu (embedded graph DB, Cypher)
- [ ] Procedural memory: versioned step sequences, dry-run / live execution
- [ ] Persistent bandit state across CLI invocations
- [ ] Tauri desktop app (macOS/Windows/Linux)
- [ ] Confidence decay background loop

### Phase 3 — Latent intelligence (JEPA / World Model)
- [ ] JEPA encoder (~5M params MLP, VICReg loss) — predicts latent target instead of cosine similarity
- [ ] World Model (~2–5M params MLP) — predicts retrieval outcomes, powers intent recommendation
- [ ] Surprise-based ingestion — replace static confidence gate with `||predicted - actual||`
- [ ] SSM/Mamba (~1–3M params) — compress episodic history to fixed-size hidden state
- [ ] All models train on CPU during idle, no GPU required

### Phase 4 — Enterprise / Docker
- [ ] Docker Compose deployment
- [ ] Multi-user governance (ACL, audit log)
- [ ] REST API layer

---

## Palantir Ontology alignment

| Palantir | TraceMind |
|---|---|
| Objects (Nouns) | `Entity` nodes |
| Links (Relationships) | `Triple` edges |
| Properties | `confidence`, `source_id`, timestamps |
| Actions / Verbs | `Procedure` + `ProcedureStep` (Phase 2) |
| Security | `GovernanceFilter` (PII + confidence gate) |
| Audit Trail | `Trace` + JSONL episodic store |
| Object Versioning | `Procedure.version` + `parent_id` |

---

## TITANS / MIRAS correspondence

| Concept | TITANS | TraceMind |
|---|---|---|
| Short-term memory | Sliding window attention | MCP context window |
| Long-term memory | Neural memory module | Graph + vector + episodic |
| Kinetic memory | Action policy head | Procedural memory (Phase 2) |
| Surprise metric | Loss gradient | `surprise_score` in Trajectory (Phase 3) |
| Retention policy | Retention gate | Confidence decay + TTL forgetting |

---

## Binary sizes (release)

```
target/release/tracemind   2.6 MB
target/release/tm-mcp      3.1 MB
```

---

*Legacy Python prototype (AgentMem v2) is preserved in `legacy/` for reference.*
