# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build --release

# Test all 10 crates
cargo test --workspace

# Test a single crate
cargo test -p tm-types
cargo test -p tm-graph
cargo test -p tm-vector
cargo test -p tm-episodic
cargo test -p tm-governance
cargo test -p tm-controller
cargo test -p tm-ingest
cargo test -p tm-retrieval
cargo test -p tm-mcp
cargo test -p tm-cli

# Run CLI binary (after build)
./target/release/tracemind ingest "<text>"
./target/release/tracemind query "<text>"
./target/release/tracemind feedback <arm_index> <reward_0_to_1>
./target/release/tracemind trace --limit 10
./target/release/tracemind status

# Run MCP server (JSON-RPC 2.0 over stdin/stdout)
./target/release/tm-mcp

# Override data directory (default: ~/.tracemind/)
TM_DATA_DIR=/tmp/tm-test ./target/release/tracemind ingest "test"
```

## Architecture

**TraceMind** is a local-only memory OS. All data lives in `~/.tracemind/` — no cloud, no telemetry.

### Crate dependency layers

```
tm-cli, tm-mcp              ← binaries / integration layer
  ├─ tm-ingest              ← orchestrates governance + graph + vector
  │   ├─ tm-governance      ← PII filter + confidence gate
  │   ├─ tm-graph           ← SQLite entity/triple store (BFS traversal)
  │   └─ tm-vector          ← 384-dim embedder + cosine search (SQLite)
  ├─ tm-retrieval           ← multi-phase recall; integrates bandit + temporal + decomposed paths
  │   ├─ tm-controller      ← UCB1 + LinUCB contextual bandit (5 arms)
  │   ├─ tm-graph
  │   ├─ tm-vector
  │   └─ tm-episodic        ← append-only JSONL trace + trajectory logs
  └─ tm-controller
      └─ (all crates depend on tm-types)

tm-types                    ← pure domain structs, zero I/O
```

### Data flow

**Ingest**: `IngestPipeline` → `GovernanceFilter` (PII regex scan) → heuristic NER → `GraphStore.upsert()` (SQLite) → `VectorStore.embed()` (SQLite) → `TraceStore.append()` (JSONL)

**Query (standard)**: `QueryPlanner.plan()` → `LinUCB.select()` picks one of 5 arms → `RetrievalEngine.query()` runs 14 phases:
1. Plan + embed + arm select
2. Vector search → ColBERT rerank → ColBERT MaxSim (arm 4) → RRA fusion
3. Fallback cascade → MMR diversity → K-hop graph expansion
4. Triple collection → episodic scan → procedures → confidence assessment

**Query (temporal)**: "what was I working on last week?" → time-range filter on entities/access log/traces → merge/dedup → rank by sim + recency

Then `LinUCB.register_reward(arm, relevance)` → `TrajectoryStore.append()` (JSONL)

### Bandit arms

| Arm | Name    | top_k | hops | episodic | colbert |
|-----|---------|-------|------|----------|---------|
| 0   | narrow  | 5     | 0    | no       | no      |
| 1   | medium  | 10    | 1    | no       | no      |
| 2   | wide    | 15    | 2    | no       | no      |
| 3   | deep    | 20    | 2    | yes      | no      |
| 4   | colbert | 10    | 1    | no       | yes     |

Bandit state persists to `~/.tracemind/bandit.json` + `linucb.json`.

### Key design constraints

- **Real ONNX embeddings**: BGE-small-en-v1.5 via fastembed (384-dim). `--hash-embed` flag for tests. `TM_EMBED_MODEL` env for model selection.
- **SQLite only**: both graph store and vector store use `rusqlite` (bundled), no separate DB process
- **Stdlib-only PII**: `tm-governance` uses only regex from stdlib — no NLP libraries
- **Trace provenance**: every ingest and query produces a `Trace` (UUID, timestamp, input hash, output entity IDs) logged to JSONL — immutable audit trail
- **Target footprint**: <200MB idle RAM, <500MB active

### MCP server tools (tm-mcp)

Exposed to Claude Code via JSON-RPC 2.0 on stdin/stdout:
- `memory_store` — ingest text
- `memory_query` — natural language query (auto-enriches temporal/reasoning/analogy)
- `get_trace` — fetch recent traces
- `list_procedures` — list stored procedures
- `memory_reason` — explicit reasoning chains
- `memory_analogies` — structural similarity
- `memory_consolidate` — memory maintenance

### Roadmap phases

- **Phase 5**: JEPA encoder, world model, surprise-based ingestion, SSM/Mamba history compression
- **Phase 6**: Docker Compose, multi-user ACL, REST API, Tauri packaging
