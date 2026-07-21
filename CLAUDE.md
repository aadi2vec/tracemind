# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build --release

# Test the whole workspace (30 crates)
cargo test --workspace

# Test individual crates
cargo test -p tm-types -p tm-graph -p tm-vector -p tm-episodic
cargo test -p tm-governance -p tm-controller -p tm-ingest -p tm-retrieval
cargo test -p tm-reason -p tm-rerank -p tm-answer
cargo test -p tm-mcp -p tm-cli -p tm-capture -p tm-tauri
cargo test -p tm-bench -p tm-bench-locomo

# CLI binary (tracemind)
./target/release/tracemind ingest "<text>"          # ingest text (or "-" for stdin)
./target/release/tracemind query "<text>"            # query memory
./target/release/tracemind feedback <arm> <reward>   # bandit feedback (0.0-1.0)
./target/release/tracemind trace --limit 10          # show recent traces
./target/release/tracemind decay --factor 0.95       # decay confidences
./target/release/tracemind proc list                 # learnable procedures
./target/release/tracemind import <path>             # bulk-ingest a directory
./target/release/tracemind status                    # bandit arm stats
./target/release/tracemind recent --limit 20         # capture event ring buffer

# MCP server (JSON-RPC 2.0 over stdin/stdout)
./target/release/tm-mcp

# Capture daemon (clipboard + shell history watcher)
./target/release/tracemind-capture

# Desktop app (Tauri 2)
./target/release/tracemind-app

# Benchmarks
./target/release/tm-bench                            # ingest/retrieval microbench
./target/release/tm-bench-ner                        # NER quality eval
./target/release/tm-bench-ner-e2e                    # round-trip ingest→query
./target/release/tm-bench-locomo \
  --dataset crates/tm-bench-locomo/fixtures/locomo-mini.json \
  --runner tracemind --real-embeddings \
  --output crates/tm-bench-locomo/baselines/v0.2-bge-mini.json

# Build with optional features
cargo build -p tm-bench-locomo --features tracemind --release   # real LoCoMo runner
cargo build -p tm-answer --features local-llm --release         # Tier 1 LLM (900MB GGUF)
cargo build -p tm-answer --features apple-fm --release          # Tier 2 Apple FM (macOS 26+)

# Override data directory (default: ~/.tracemind/)
TM_DATA_DIR=/tmp/tm-test ./target/release/tracemind ingest "test"

# Force hash embedder (offline / deterministic tests)
./target/release/tracemind --hash-embed query "test"
```

## Architecture

**TraceMind** is a local-only memory OS. All data lives in `~/.tracemind/` — no cloud, no telemetry. 17-crate Rust workspace; ships as a CLI, MCP server, capture daemon, and Tauri desktop app.

### Crate map (17 crates)

```
binaries/integration ─ tm-cli, tm-mcp, tm-capture, tm-tauri
                          │
answer layer ──────────── tm-answer  (Tier 0/1/2 dispatcher; Phase 3)
                          │
retrieval ─────────────── tm-retrieval ── tm-rerank (ColBERT MaxSim)
                          │             ── tm-reason (chains, analogies, causal trace, consolidator)
                          │             ── tm-controller (UCB1 + LinUCB, 5 arms)
                          │
ingest ────────────────── tm-ingest ── tm-governance (PII + confidence gate)
                          │         ── (GLiNER NER via ONNX, optional auto-download)
                          │
storage ───────────────── tm-graph (SQLite, KG-R1 4-action traversal)
                          tm-vector (BGE-small ONNX, 384d, cosine)
                          tm-episodic (TraceStore, TrajectoryStore, ProcedureStore, RecentStore)
                          │
benchmarks (standalone) ─ tm-bench, tm-bench-locomo
                          │
core types ────────────── tm-types  (zero I/O; everyone depends on this)
```

### Data flow

**Ingest** (`tm-ingest::IngestPipeline`):
governance gate (PII + confidence) → heuristic NER (or `GlinerExtractor` if available) → `GraphStore.upsert()` (entities + typed triples) → `VectorStore.embed()` (BGE-small via fastembed) → `TraceStore.append()` Ingest event → `RecentStore.append()` ring buffer for capture-feedback

**Query** (`tm-retrieval::RetrievalEngine`):
1. `QueryPlanner` classifies (standard / temporal / decomposed / reasoning / analogy)
2. `LinUcbBandit.select()` picks one of 6 arms (see table) — preceded by `QueryPlanner` memory-routing gate (`should_retrieve: bool`)
3. Pipeline phases (varies by arm):
   - vector search → ColBERT rerank (arm 4 also runs MaxSim) → RRA fusion
   - signal hybrid path (raw captures via `graph.search_signals`)
   - K-hop graph expansion → MMR diversity
   - episodic scan (arm 3 only)
   - reasoning chains / analogy solver / temporal range filter (planner-routed)
   - 1-hop related entities (TM-UX-001) → confidence assessment
4. Append `Retrieve` trace, `LinUcbBandit.register_reward(arm, relevance)`, log `Trajectory`

**Answer** (`tm-answer::TieredAnswerer`, Phase 3 — scaffolded):
`AnswerRequest{ question, grounding, task, max_output_tokens, preferred_tier }` →
`TieredAnswerer.answer()` selects backend by `BackendAvailability` + `TaskKind::is_structured()` →
returns `AnswerResponse{ tier, text, citations, latency_ms }`

### Bandit arms (`tm-controller`)

| Arm | Name    | top_k | hops | episodic | colbert |
|-----|---------|-------|------|----------|---------|
| 0   | narrow  | 5     | 0    | no       | no      |
| 1   | medium  | 10    | 1    | no       | no      |
| 2   | wide    | 15    | 2    | no       | no      |
| 3   | deep    | 20    | 2    | yes      | no      |
| 4   | colbert | 10    | 1    | no       | yes     |

State persists to `~/.tracemind/bandit.json` + `linucb.json`.

### Tier system (`tm-answer`, Phase 3)

| Tier | Backend                | Cost          | Status      | Best for                                      |
|------|------------------------|---------------|-------------|-----------------------------------------------|
| 0    | `ExtractiveBackend`    | 0 MB          | shipped     | always-on fallback; templated synthesis       |
| 1    | `LocalLlmBackend`      | ~900 MB GGUF  | scaffolded  | structured extraction, contradiction, short Q&A (Qwen 2.5 1.5B Q4_K_M, llama.cpp via `llama-cpp-2`) |
| 2    | `AppleFmBackend`       | 0 MB          | scaffolded  | open-ended synthesis on macOS 26 + Apple Silicon (Apple FoundationModels) |

Selection: structured tasks prefer Tier 1; open-ended prefers Tier 2 → Tier 1 → Tier 0. Tier 0 always succeeds.

### Key design constraints

- **Real ONNX embeddings**: BGE-small-en-v1.5 via fastembed (384-dim). `--hash-embed` for tests. Model bundled in `~/.tracemind/models/` or auto-downloaded from HF Hub on first use (TM-NLP-005).
- **ColBERT rerank always on**: `tm-rerank` (mxbai-edge-colbert-v0-17m, ~35MB) auto-downloads on first use; no opt-out.
- **SQLite only**: both `tm-graph` and `tm-vector` use `rusqlite` (bundled).
- **Stdlib-only PII**: `tm-governance` uses regex only — no NLP libraries.
- **Trace provenance**: every ingest + query produces a `Trace` (UUID, timestamp, content hash, entity IDs) logged to `~/.tracemind/traces.jsonl` — immutable audit trail.
- **Footprint (revised 2026-04-26 — no hard cap above Tier 0)**: per-platform tiers — laptop ~1.6GB active with Tier-1 (Qwen 2.5 1.5B Q4); mobile ~600MB with Qwen 0.5B Q4; iOS 26+ via Apple FoundationModels (OS-managed); Tier-0 fallback stays under 200MB idle for browser / low-end. See `docs/DESIGN.md` §3.7.
- **Local-only by default**: no network in the request path. Network is used only for opt-in model downloads and the *opt-in encrypted-cloud Tier* (low-end devices only, off by default, user-toggleable). See `docs/DESIGN.md` §4.

### MCP tools (`tm-mcp`, JSON-RPC 2.0 over stdio)

| Tool                  | Purpose                                                        |
|-----------------------|----------------------------------------------------------------|
| `memory_store`        | ingest text + return proactive context (TM-UX-001 Phase C)     |
| `memory_query`        | NL query → entities/triples/related/explanation/reasoning      |
| `get_trace`           | recent ingest/retrieve traces with full audit detail           |
| `list_procedures`     | learnable procedures (currently empty stub)                    |
| `memory_reason`       | multi-hop chain exploration (`ChainBuilder`)                   |
| `memory_analogies`    | structural similarity (`AnalogySolver`)                        |
| `memory_consolidate`  | decay + merge maintenance (`Consolidator`)                     |

### Data layout (`~/.tracemind/`)

```
~/.tracemind/
├── memory.db                    # SQLite graph + vector
├── memory.db-wal, memory.db-shm
├── traces.jsonl                 # immutable trace log
├── recent.jsonl                 # capture ring buffer
├── bandit.json                  # UCB1 state
├── linucb.json                  # contextual bandit state
└── models/                      # bundled or downloaded
    ├── bge-384-v1.5/
    ├── mxbai-colbert/
    └── gliner-*-multi/          # optional
```

### Benchmarks

- **`tm-bench`** — ingest + retrieval microbenchmarks
- **`tm-bench-ner`** — GLiNER NER quality eval against labeled sets
- **`tm-bench-ner-e2e`** — end-to-end round-trip
- **`tm-bench-locomo`** — published LoCoMo scoring harness (token F1 + EM, 5 categories: single_hop / multi_hop / temporal / open_domain / adversarial). CI gate fails any PR that drops > 0.5 F1. Current mini-set baseline: **F1 49.27 / EM 30.00** (v0.4, 2026-05-09, both BGE and hash; lift from `tm-bench-locomo::extract` Tier-0 span extractors). See `docs/DESIGN.md` §13.

### Roadmap

See `docs/DESIGN.md` for architecture and decisions, `docs/TASKS.md` for the prioritized implementation task list.
