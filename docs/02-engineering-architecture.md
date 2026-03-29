# TraceMind: Engineering Architecture Document

**Version:** 1.0
**Author:** Engineering Team
**Date:** 2026-03-28
**Status:** Approved for Phase 1 implementation
**Audience:** Senior engineers, systems architects

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Component Deep Dives](#2-component-deep-dives)
3. [Data Models](#3-data-models)
4. [Rust Crate Structure](#4-rust-crate-structure)
5. [Embedding Pipeline](#5-embedding-pipeline)
6. [Memory Controller FSM](#6-memory-controller-fsm)
7. [Retrieval Algorithm](#7-retrieval-algorithm)
8. [Learning Pipeline](#8-learning-pipeline)
8b. [JEPA/World Model/SSM Training Pipeline](#8b-jepaworldmodelssm-training-pipeline)
9. [IPC and Communication](#9-ipc-and-communication)
10. [Browser Extension Architecture](#10-browser-extension-architecture)
11. [Claude Code Integration](#11-claude-code-integration)
12. [Storage Layout](#12-storage-layout)
13. [Performance Budget](#13-performance-budget)
14. [Build and Distribution](#14-build-and-distribution)
15. [Testing Strategy](#15-testing-strategy)
16. [Migration Plan](#16-migration-plan)

---

## 1. System Overview

TraceMind is a local-only, privacy-first memory operating system for AI agents and humans. All computation stays on-device. The system is implemented as a Rust monolith with a Tauri v2 desktop shell, embedded databases, and a local ONNX embedding pipeline. No network calls are made for core functionality. No JVM. No server processes.

### 1.1 Architecture Diagram

```
+=====================================================================+
|                        CAPTURE LAYER                                |
|  +-------------+ +-----------+ +---------------+ +--------------+   |
|  | Browser Ext | | Clipboard | | Claude Code   | | File Watcher |   |
|  | (Manifest   | | Monitor   | | Hooks + MCP   | | (notify-rs)  |   |
|  |  V3, IPC)   | | (OS APIs) | | (stdin/stdout)| | (inotify/    |   |
|  +------+------+ +-----+-----+ +-------+-------+ | FSEvents/    |   |
|         |              |               |          | ReadDirChg)  |   |
|         +-------+------+-------+-------+          +---------+----+   |
|                 |              |                            |         |
|                 v              v                            v         |
+=====================================================================+
|                   CANONICALIZATION LAYER (Rust)                      |
|  +------------------+  +------------------+  +------------------+   |
|  | Entity Extractor |  | Triple Generator |  | Embedding Engine |   |
|  | (regex + NER     |  | (LLM-assisted   |  | (ONNX Runtime +  |   |
|  |  heuristics)     |  |  or rule-based)  |  |  MiniLM-L6-v2)  |   |
|  +--------+---------+  +--------+---------+  +--------+---------+   |
|           |                     |                      |             |
|           +----------+----------+----------+-----------+             |
|                      v                     v                         |
|              +---------------+    +-----------------+                |
|              | Denoiser      |    | Dedup / Merge   |                |
|              | (low-signal   |    | (entity coref)  |                |
|              |  filter)      |    |                 |                |
|              +-------+-------+    +--------+--------+                |
|                      +----------+----------+                         |
+=====================================================================+
|                   GOVERNANCE FUNNEL (Rust)                           |
|  +-------------+ +------------------+ +------------+ +-----------+  |
|  | PII Filter  | | Schema Validator | | User Rules | | Consent   |  |
|  | (regex +    | | (type check,     | | Engine     | | Verifier  |  |
|  |  bloom)     | |  required fields)| | (TOML DSL) | | (per-src) |  |
|  +------+------+ +--------+---------+ +-----+------+ +-----+-----+ |
|         +----------+-------+-----------+-----+-------+------+       |
|                                v                                     |
+=====================================================================+
|               STRUCTURED MEMORY LAYER (Embedded DBs)                |
|  +-----------------+ +----------------+ +----------+ +-----------+  |
|  | Entity Graph    | | Vector Store   | | Cluster  | | Episodic  |  |
|  | Kuzu (embedded  | | LanceDB        | | Store    | | Trace     |  |
|  |  C++ via FFI)   | | (Rust-native   | | HDBSCAN  | | Store     |  |
|  | ~5MB footprint  | |  columnar)     | | (Rust    | | (Parquet  |  |
|  | ACID, typed     | | ~20MB          | |  port)   | |  append-  |  |
|  | property graph  | | ANN index      | | SQLite   | |  only)    |  |
|  +---------+-------+ +--------+-------+ | backing  | +-----------+  |
|            |                  |          +----------+                |
+=====================================================================+
|                   MEMORY CONTROLLER                                  |
|  +------------------+  +------------------+  +------------------+   |
|  | FSM Engine       |  | UCB Bandit       |  | Store/Defer Gate |   |
|  | (deterministic   |  | (arm selection   |  | (confidence      |   |
|  |  state machine)  |  |  for retrieval)  |  |  threshold +     |   |
|  |                  |  |                  |  |  TTL decay)      |   |
|  +--------+---------+  +--------+---------+  +--------+---------+   |
|           +----------+----------+----------+-----------+             |
+=====================================================================+
|                   RETRIEVAL ENGINE                                   |
|  Phase 1: Vector ANN --> Phase 2: Graph --> Phase 3: Cluster --> Phase 4: Procedural |
|  (LanceDB top-k)         Traversal         Expansion            Recall              |
|                              (BFS, bounded)      (soft membership)  |
|  Recall budget: max 50 facts total, latency < 100ms p95             |
+=====================================================================+
|               REASONING & INTENT LAYER                              |
|  +---------------------+ +--------------------+ +----------------+  |
|  | Context Continuation| | Pattern Detection  | | Proactive      |  |
|  | (predict next need  | | (cross-session     | | Surfacing      |  |
|  |  from recent trace) | |  recurrence)       | | (idle push)    |  |
|  +---------------------+ +--------------------+ +----------------+  |
+=====================================================================+
|                   OUTPUT INTERFACES                                  |
|  +----------+ +------------+ +----------+ +---------+ +----------+  |
|  | Tauri    | | MCP Server | | Claude   | | Local   | | CLI      |  |
|  | Desktop  | | (JSON-RPC  | | Code     | | REST    | | (tmind)  |  |
|  | App      | |  over      | | Hooks    | | (Axum,  | |          |  |
|  | (webview)| |  stdio)    | | (bash)   | | 127.0.0.1)         |  |
|  +----------+ +------------+ +----------+ +---------+ +----------+  |
+=====================================================================+
|               LEARNING LOOP (Background, Idle-time)                 |
|  Phase 1: UCB arm updates from feedback signals                     |
|  Phase 2: Trajectory storage -> lightweight policy net (MLP)        |
|  Phase 3: RL-based memory CRUD (Memory-R1, offline GRPO)            |
+=====================================================================+
```

### 1.2 Critical Constraints

| Constraint | Target | Hard Limit |
|-----------|--------|------------|
| RAM idle | <150MB | 200MB |
| RAM active | <400MB | 500MB |
| Install size (Phase 1-2) | <200MB | 250MB |
| Install size (Phase 3, +JEPA/WM/SSM) | <260MB | 310MB |
| Binary size (Rust) | <40MB | 50MB |
| Embedding model | 80MB | 100MB |
| Startup time | <2s | 3s |
| Retrieval latency p95 | <80ms | 100ms |
| Embedding latency (single) | <30ms | 50ms |
| Background CPU (idle) | <2% | 5% |

### 1.3 Platform Matrix

| Platform | Architecture | IPC Mechanism | File Watcher | Notes |
|----------|-------------|---------------|--------------|-------|
| macOS | aarch64 (M1+) | Unix domain socket | FSEvents | Primary dev target |
| macOS | x86_64 | Unix domain socket | FSEvents | Rosetta 2 fallback |
| Windows | x86_64 | Named pipes | ReadDirectoryChangesW | NTFS paths |
| Linux | x86_64 | Unix domain socket | inotify | Wayland + X11 |
| Linux | aarch64 | Unix domain socket | inotify | Raspberry Pi 5+ |

---

## 2. Component Deep Dives

### 2.1 Capture Layer

The capture layer is a collection of independent, stateless data sources that emit `CaptureEvent` messages into a shared channel. Each source runs in its own async task.

**Interface (all sources implement):**

```rust
#[async_trait]
pub trait CaptureSource: Send + Sync {
    /// Human-readable source identifier (e.g., "browser", "clipboard")
    fn source_id(&self) -> &str;

    /// Start emitting events into the provided channel.
    /// Returns when the source is shut down.
    async fn run(&self, tx: mpsc::Sender<CaptureEvent>) -> Result<()>;

    /// Graceful shutdown signal.
    async fn shutdown(&self);
}
```

**CaptureEvent schema:**

```rust
pub struct CaptureEvent {
    pub id: Uuid,
    pub source: SourceType,        // Browser, Clipboard, ClaudeCode, FileWatch
    pub timestamp: i64,            // Unix millis
    pub content: CaptureContent,   // Enum: Text(String), Url(UrlCapture), File(PathBuf)
    pub metadata: HashMap<String, String>,
}
```

**Source-specific behavior:**

| Source | Mechanism | Event Rate | Dedup Window |
|--------|-----------|-----------|--------------|
| Browser Extension | IPC message via Unix socket/named pipe | ~1/sec browsing | 5s URL dedup |
| Clipboard Monitor | OS polling (500ms interval) | ~0.2/sec average | Content hash dedup |
| Claude Code Hooks | stdin/stdout JSON lines | Burst on session | Session ID dedup |
| File Watcher | notify-rs (FSEvents/inotify) | Debounced 2s | Path + mtime dedup |

**Error handling:** Each source has independent retry with exponential backoff (base 1s, max 60s, jitter). Source failure does not affect other sources. All errors logged with `tracing` spans.

**Backpressure:** Channel bounded at 1024 events. If full, oldest events dropped with warning counter incremented. Counter exposed via metrics.

### 2.2 Canonicalization Layer

Transforms raw `CaptureEvent` into structured `CanonicalFact` records. Runs as a pipeline of sequential stages.

**Pipeline stages:**

```
CaptureEvent
  -> EntityExtractor     (produces Vec<Entity>)
  -> TripleGenerator     (produces Vec<Triple>)
  -> EmbeddingEngine     (attaches Vec<f32> to each entity/triple)
  -> Denoiser            (filters low-information triples)
  -> DedupMerger         (coreference resolution, entity merge)
  -> CanonicalFact       (final output)
```

**Entity Extraction strategy (no LLM required):**

1. Regex patterns for structured entities: emails, URLs, file paths, dates, version numbers, IP addresses
2. Capitalization heuristics for proper nouns (consecutive capitalized words not at sentence start)
3. Domain dictionaries (loaded from TOML config): programming languages, frameworks, tools
4. Coreference rules: "it" -> most recent entity of matching type in 3-sentence window

**Triple Generation:**

Phase 1 (rule-based): Subject-verb-object extraction via dependency parse heuristics. Common patterns:
- `X uses Y` -> (X, USES, Y)
- `X is a Y` -> (X, IS_A, Y)
- `X depends on Y` -> (X, DEPENDS_ON, Y)

Phase 2 (LLM-assisted, optional): Local LLM call for complex sentences. LLM is advisory only -- output validated against schema before acceptance.

**Denoiser thresholds:**

| Signal | Threshold | Action |
|--------|-----------|--------|
| Entity name length | <2 chars | Drop |
| Triple confidence | <0.3 | Drop |
| Content entropy | <1.0 bits/char | Drop (repetitive text) |
| Duplicate ratio in batch | >80% overlap | Merge into single fact |

**Error handling:** Malformed events logged and skipped. Pipeline continues on partial extraction failure (e.g., embedding fails but entities succeed -- entities stored without vectors, queued for retry).

### 2.3 Governance Funnel

All facts pass through governance before reaching storage. Governance is synchronous and blocking -- no fact bypasses it.

**PII Filter:**

```rust
pub struct PiiFilter {
    patterns: Vec<CompiledRegex>,   // SSN, credit card, phone, email
    bloom: BloomFilter,             // Known PII tokens from user config
    action: PiiAction,              // Redact | Drop | Tag
}
```

Built-in patterns: US SSN (`\d{3}-\d{2}-\d{4}`), credit card (Luhn-validated), phone (E.164), email. User can add custom patterns in `~/.tracemind/governance.toml`.

**Schema Validator:**

Every fact must conform to the type system:
- Entity: requires `name` (non-empty), `type` (from allowed set or user-defined)
- Triple: requires valid `subject`, `predicate`, `object`; `confidence` in [0.0, 1.0]
- Embedding: dimension must equal 384 (MiniLM-L6-v2 output dim)

**User Rules Engine:**

Rules expressed in TOML:

```toml
[[rules]]
name = "block_social_media"
condition = { source = "browser", url_contains = "twitter.com" }
action = "drop"

[[rules]]
name = "redact_api_keys"
condition = { content_matches = "(?i)(api[_-]?key|secret)[=:]\\s*\\S+" }
action = "redact"
```

Rules evaluated top-to-bottom. First matching rule wins. Default action: `allow`.

**Consent Verifier:**

Per-source consent stored in `~/.tracemind/consent.toml`:

```toml
[consent]
browser = true
clipboard = true
claude_code = true
file_watcher = false   # user has not opted in
```

Events from non-consented sources dropped at funnel entry.

**Audit log:** Every governance decision (allow, drop, redact) written to append-only log at `~/.tracemind/data/audit.parquet` with columns: `[timestamp, event_id, source, rule_name, action, reason]`.

### 2.4 Structured Memory Layer

Four embedded stores, each serving a different access pattern.

#### 2.4.1 Entity Graph (Kuzu)

Kuzu is an embedded graph database (C++ core, no server process). Accessed via Rust FFI bindings.

**Schema:**

```cypher
CREATE NODE TABLE Entity (
    id STRING PRIMARY KEY,
    name STRING,
    entity_type STRING,
    description STRING,
    confidence FLOAT,
    created_at INT64,
    updated_at INT64,
    ttl_hours FLOAT,
    source_id STRING
);

CREATE NODE TABLE Procedure (
    id STRING PRIMARY KEY,
    name STRING,
    description STRING,
    steps STRING,           -- JSON-encoded Vec<ProcedureStep>
    version INT32,
    deprecated BOOLEAN,
    confidence FLOAT,
    created_at INT64
);

CREATE REL TABLE RELATED_TO (
    FROM Entity TO Entity,
    predicate STRING,
    confidence FLOAT,
    timestamp INT64,
    source_id STRING
);

CREATE REL TABLE HAS_PROCEDURE (
    FROM Entity TO Procedure,
    relevance FLOAT
);
```

**Access patterns and indexes:**

| Query Pattern | Index | Expected Latency |
|--------------|-------|-----------------|
| Lookup by entity ID | Primary key | <1ms |
| Fuzzy name search | FTS index on `name` | <5ms |
| 1-hop neighbors | Adjacency scan | <3ms for degree <100 |
| 2-hop neighbors | Adjacency scan x2 | <10ms |
| Triplets by source_id | B-tree on `source_id` | <2ms |

**Fallback:** If Kuzu FFI fails to initialize (rare edge case on unusual platforms), fall back to SQLite adjacency list representation:

```sql
CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT,
    description TEXT,
    confidence REAL DEFAULT 1.0,
    created_at INTEGER,
    updated_at INTEGER
);
CREATE INDEX idx_entity_name ON entities(name);

CREATE TABLE edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id TEXT REFERENCES entities(id),
    predicate TEXT NOT NULL,
    object_id TEXT REFERENCES entities(id),
    confidence REAL DEFAULT 1.0,
    timestamp INTEGER,
    source_id TEXT
);
CREATE INDEX idx_edge_subject ON edges(subject_id);
CREATE INDEX idx_edge_object ON edges(object_id);
CREATE INDEX idx_edge_source ON edges(source_id);
```

#### 2.4.2 Vector Store (LanceDB)

LanceDB is an embedded columnar vector database written in Rust. Stores embeddings alongside metadata.

**Table schema:**

```
Table: memories
Columns:
  id:         String (UUID)
  text:       String (original text chunk)
  embedding:  FixedSizeList[Float32, 384]  (MiniLM-L6-v2 dimension)
  source:     String (source type)
  source_id:  String (links to graph entity/triple)
  timestamp:  Int64 (Unix millis)
  metadata:   String (JSON blob for extensibility)

Index: IVF_PQ on embedding column
  - nlist: 128 (number of partitions)
  - nbits: 8 (PQ quantization bits)
  - Retraining trigger: every 10,000 new vectors
```

**Search parameters:**

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Top-k default | 10 | Balance recall vs. latency |
| Top-k max | 50 | Hard budget cap |
| nprobes | 16 | ~90% recall@10 on 100k vectors |
| Refine factor | 2 | Re-rank PQ results with exact distance |
| Distance metric | Cosine | Normalized embeddings from MiniLM |

**Memory-mapped access:** LanceDB memory-maps its data files. Only actively queried partitions loaded into RAM. Estimated RSS contribution: ~15MB for 100k vectors.

#### 2.4.3 Cluster Store (HDBSCAN + SQLite)

Maintains soft cluster assignments for all entities. Used for Phase 3 cluster expansion in retrieval.

**HDBSCAN parameters:**

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| min_cluster_size | 5 | Avoid micro-clusters |
| min_samples | 3 | Noise tolerance |
| cluster_selection_method | eom (excess of mass) | Better for varying density |
| metric | cosine | Matches embedding space |

**Incremental update strategy:**

Full HDBSCAN is O(n^2). For incremental updates:
1. New entities accumulate in a staging buffer (max 500 entities).
2. When buffer fills OR on idle-time trigger, run approximate update:
   a. Assign new entities to nearest existing cluster centroid (cosine similarity > 0.7).
   b. If no cluster matches, add to noise set.
3. Full re-cluster runs weekly or when noise set exceeds 20% of total entities.

**SQLite backing table:**

```sql
CREATE TABLE cluster_assignments (
    entity_id TEXT PRIMARY KEY,
    cluster_id INTEGER NOT NULL,        -- -1 = noise
    membership_score REAL NOT NULL,     -- [0.0, 1.0]
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_cluster ON cluster_assignments(cluster_id);

CREATE TABLE cluster_centroids (
    cluster_id INTEGER PRIMARY KEY,
    centroid BLOB NOT NULL,             -- 384 x f32, little-endian
    member_count INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

#### 2.4.4 Episodic Trace Store (Parquet)

Append-only columnar store for full decision provenance. Each trace is one row.

**Parquet schema (arrow-rs):**

```
Schema:
  trace_id:              Utf8
  task_id:               Utf8
  input_query:           Utf8
  retrieved_memory_ids:  List<Utf8>
  retrieval_arm:         Utf8
  procedure_ids:         List<Utf8>
  reasoning_steps:       List<Utf8>
  final_decision:        Utf8
  confidence:            Float64
  outcome:               Utf8          ("success" | "failure" | "unknown")
  reward_signal:         Float64
  feedback_score:        Float64       (nullable)
  feedback_correction:   Utf8          (nullable)
  timestamp:             Int64
```

**File rotation:** New Parquet file per day, named `traces_YYYYMMDD.parquet`. Typical row size: ~2KB compressed. At 1000 traces/day = ~2MB/day, ~60MB/month.

**Compaction:** Monthly compaction merges daily files into one. Old daily files deleted after merge verified.

---

## 3. Data Models

### 3.1 Entity Graph Schema

```
Entity {
    id:          UUID (v7, time-sortable)
    name:        String (1..256 chars, normalized lowercase)
    entity_type: Enum { Person, Organization, Project, Technology,
                        Concept, File, URL, Event, Procedure, Custom(String) }
    description: Option<String> (max 4096 chars)
    confidence:  f64 (0.0..=1.0)
    created_at:  i64 (Unix millis)
    updated_at:  i64 (Unix millis)
    ttl_hours:   Option<f64> (None = permanent)
    source_id:   Option<UUID> (vector store cross-reference)
}

Triple {
    id:          UUID (v7)
    subject_id:  UUID -> Entity
    predicate:   String (normalized, e.g., "USES", "DEPENDS_ON", "IS_A")
    object_id:   UUID -> Entity
    confidence:  f64 (0.0..=1.0)
    timestamp:   i64 (Unix millis)
    source_id:   Option<UUID>
}

Procedure {
    id: UUID,
    name: String,
    version: u32,
    trigger: String,          // natural language trigger pattern
    steps: Vec<ProcedureStep>,
    linked_entities: Vec<EntityId>,
    confidence: f64,
    status: enum { Active, Reinforced, Degraded, Deprecated, Revised },
    parent_version: Option<UUID>,  // points to previous version
    created_at: DateTime,
    updated_at: DateTime,
}

ProcedureStep {
    action: String,
    params: HashMap<String, Value>,
    expected_outcome: String,
    timeout_ms: u64,
}
```

### 3.2 Trace Schema (Episodic)

```
ContextTrace {
    trace_id:             UUID (v7)
    task_id:              String
    input_query:          String
    retrieved_memory_ids: Vec<UUID>
    retrieved_memories:   Vec<MemorySnippet>     // serialized at capture time
    graph_paths:          Vec<Vec<UUID>>          // entity ID chains
    retrieval_arm:        String                  // bandit arm name used
    procedure_ids:        Vec<UUID>
    reasoning_steps:      Vec<String>
    final_decision:       String
    confidence:           f64
    outcome:              Outcome { Success, Failure, Unknown }
    reward_signal:        f64                     // computed from outcome + feedback
    feedback:             Option<Feedback>
    timestamp:            i64
    duration_ms:          u64                     // wall-clock time for this trace
}
```

### 3.3 Trajectory Schema (Learning)

Trajectories are sequences of (state, action, reward) tuples used for offline RL training.
Extended in Phase 3 to support JEPA, World Model, and SSM training targets.

```rust
struct Trajectory {
    trace_id: Uuid,
    session_id: Uuid,
    timestamp: DateTime<Utc>,

    // State (input to JEPA/WM)
    context_embedding: Vec<f32>,          // 384-dim from MiniLM
    memory_snapshot_hash: u64,            // seahash of retrieved memory IDs
    user_activity_type: ActivityType,     // Browser, ClaudeCode, Clipboard, etc.
    active_entity_ids: Vec<Uuid>,         // entities in current context

    // Action (what the system did)
    retrieval_arm: RetrievalArm,          // narrow/medium/wide/deep
    memory_ids_retrieved: Vec<Uuid>,
    store_decisions: Vec<StoreDecision>,   // (entity_id, stored: bool, confidence)
    procedure_executed: Option<Uuid>,

    // Outcome (ground truth for training)
    user_feedback: Option<f64>,           // [-1.0, 1.0]
    task_success: Option<bool>,
    correction_applied: bool,

    // World Model targets (null until Phase 3, then filled retroactively)
    predicted_outcome: Option<Vec<f32>>,  // WM prediction at decision time
    actual_outcome_embedding: Option<Vec<f32>>,  // actual outcome encoded
    surprise_score: Option<f64>,          // |predicted - actual|
}
```

**Backward compatibility:** Phase 1/2 trajectories omit the World Model target fields (all `None`). Phase 3 backfill job populates `actual_outcome_embedding` and `surprise_score` retroactively from stored episodic traces.

**Legacy sub-types (still used for RL policy training):**

```
TrajectoryStep {
    step_index:     u32
    state:          StateVector          // encoded context features
    action:         MemoryAction         // Store | Link | Retrieve | Forget | Defer
    action_params:  HashMap<String, f64> // e.g., {"depth": 2, "breadth": 10}
    reward:         f64
    next_state:     StateVector
    timestamp:      i64
}

StateVector {
    query_embedding:       [f32; 384]
    graph_density:         f32          // edges / entities ratio in local subgraph
    cluster_entropy:       f32          // Shannon entropy of cluster distribution
    memory_age_mean:       f32          // mean age (hours) of candidate memories
    memory_age_std:        f32
    confidence_mean:       f32
    retrieval_arm_onehot:  [f32; 4]     // one-hot for current arm
    session_step:          f32          // normalized position in session
    total_features:        393          // 384 + 9 scalar features
}
```

**Storage format:** Trajectories stored as Parquet in `~/.tracemind/data/trajectories/`. Partitioned by month. Typical trajectory: 5-20 steps, ~8KB compressed.

---

## 4. Rust Crate Structure

```
tracemind/
  Cargo.toml                    # workspace root
  crates/
    tm-core/                    # Core types, traits, error types
      Cargo.toml                # deps: serde, uuid, thiserror
      src/
        lib.rs
        types.rs                # Entity, Triple, CaptureEvent, etc.
        traits.rs               # CaptureSource, StorageBackend, etc.
        error.rs                # TraceMindError enum

    tm-capture/                 # Capture layer sources
      Cargo.toml                # deps: tm-core, tokio, notify (file watcher)
      src/
        lib.rs
        clipboard.rs            # OS clipboard polling
        file_watcher.rs         # notify-rs based watcher
        claude_code.rs          # stdin/stdout JSON line protocol
        browser_ipc.rs          # Unix socket / named pipe listener

    tm-canon/                   # Canonicalization pipeline
      Cargo.toml                # deps: tm-core, regex, unicode-segmentation
      src/
        lib.rs
        entity_extract.rs       # Regex + heuristic NER
        triple_gen.rs           # Rule-based triple generation
        denoiser.rs             # Low-signal filtering
        dedup.rs                # Entity coreference / merge

    tm-governance/              # Governance funnel
      Cargo.toml                # deps: tm-core, toml, regex
      src/
        lib.rs
        pii_filter.rs           # PII detection and redaction
        schema_validator.rs     # Type and constraint validation
        rules_engine.rs         # TOML rule evaluation
        consent.rs              # Per-source consent checks
        audit.rs                # Append-only audit log writer

    tm-graph/                   # Entity graph (Kuzu + SQLite fallback)
      Cargo.toml                # deps: tm-core, kuzu (sys crate), rusqlite
      src/
        lib.rs
        kuzu_backend.rs         # Kuzu FFI wrapper
        sqlite_backend.rs       # SQLite adjacency list fallback
        graph_ops.rs            # Common graph operations

    tm-vector/                  # Vector store (LanceDB)
      Cargo.toml                # deps: tm-core, lancedb
      src/
        lib.rs
        store.rs                # LanceDB table management
        search.rs               # ANN search with parameters

    tm-cluster/                 # Clustering (HDBSCAN)
      Cargo.toml                # deps: tm-core, rusqlite, ndarray
      src/
        lib.rs
        hdbscan.rs              # HDBSCAN implementation (Rust port or C++ FFI)
        incremental.rs          # Incremental cluster assignment
        store.rs                # SQLite cluster persistence

    tm-embedding/               # ONNX embedding pipeline
      Cargo.toml                # deps: ort (ONNX Runtime Rust bindings)
      src/
        lib.rs
        model.rs                # Model loading, session management
        tokenizer.rs            # Tokenizer (HuggingFace tokenizers-rs)
        inference.rs            # Batch inference, quantization
        pool.rs                 # Thread-safe model pool

    tm-trace/                   # Episodic trace store
      Cargo.toml                # deps: tm-core, arrow-rs, parquet
      src/
        lib.rs
        writer.rs               # Append-only Parquet writer
        reader.rs               # Trace query / scan
        compaction.rs           # File rotation and merge

    tm-controller/              # Memory controller (FSM + bandit)
      Cargo.toml                # deps: tm-core, rand
      src/
        lib.rs
        fsm.rs                  # Finite state machine
        bandit.rs               # UCB bandit arms
        gate.rs                 # Store/defer decision logic
        decay.rs                # Confidence decay / TTL

    tm-retrieval/               # 3-phase retrieval engine
      Cargo.toml                # deps: tm-core, tm-graph, tm-vector, tm-cluster
      src/
        lib.rs
        pipeline.rs             # Orchestrates 3 phases
        budget.rs               # Recall budget enforcement
        ranker.rs               # Final re-ranking

    tm-learning/                # Learning loop and RL
      Cargo.toml                # deps: tm-core, tm-trace, ndarray, tch (optional)
      src/
        lib.rs
        trajectory.rs           # Trajectory recording and storage
        bandit_update.rs        # UCB reward propagation
        policy_net.rs           # Lightweight MLP policy
        trainer.rs              # Offline training loop (idle-time)

    tm-procedural/              # Procedural memory (kinetic actions)
      Cargo.toml                # deps: tm-core, tm-graph, serde_json, uuid
      src/
        lib.rs
        store.rs                # Procedure CRUD in entity graph (HAS_PROCEDURE edges)
        executor.rs             # Procedure execution engine (dry-run / live modes)
        lifecycle.rs            # Status transitions: Active -> Reinforced -> Degraded -> Deprecated -> Revised
        versioning.rs           # Procedure version chain (parent_version linkage, diff)

    tm-reasoning/               # Reasoning & intent layer
      Cargo.toml                # deps: tm-core, tm-retrieval, tm-trace
      src/
        lib.rs
        continuation.rs         # Context continuation prediction
        patterns.rs             # Cross-session pattern detection
        proactive.rs            # Idle-time proactive surfacing

    tm-mcp/                     # MCP server implementation
      Cargo.toml                # deps: tm-core, serde_json, tokio
      src/
        lib.rs
        server.rs               # JSON-RPC over stdio
        handlers.rs             # Tool implementations
        protocol.rs             # MCP message types

    tm-ipc/                     # Cross-platform IPC
      Cargo.toml                # deps: tm-core, tokio, capnp (Cap'n Proto)
      src/
        lib.rs
        unix_socket.rs          # Unix domain socket (macOS/Linux)
        named_pipe.rs           # Named pipe (Windows)
        protocol.rs             # Cap'n Proto schema bindings
        router.rs               # Message routing

    tm-app/                     # Tauri application (desktop shell)
      Cargo.toml                # deps: tauri, tm-core, tm-ipc, + all tm-* crates
      src/
        main.rs                 # Tauri entry point
        commands.rs             # Tauri command handlers (frontend -> Rust)
        state.rs                # Application state management
        tray.rs                 # System tray integration

    tm-cli/                     # CLI binary
      Cargo.toml                # deps: clap, tm-core, tm-ipc
      src/
        main.rs                 # CLI entry point

  frontend/                     # SolidJS/Svelte frontend
    package.json
    src/
      ...

  extensions/
    chrome/                     # Chrome extension (Manifest V3)
      manifest.json
      ...

  models/                       # ONNX model files (gitignored, downloaded at build)
    all-MiniLM-L6-v2.onnx      # ~80MB

  config/                       # Default configuration files
    governance.toml
    consent.toml
    rules.toml
```

**Workspace Cargo.toml (root):**

```toml
[workspace]
resolver = "2"
members = [
    "crates/tm-core",
    "crates/tm-capture",
    "crates/tm-canon",
    "crates/tm-governance",
    "crates/tm-graph",
    "crates/tm-vector",
    "crates/tm-cluster",
    "crates/tm-embedding",
    "crates/tm-trace",
    "crates/tm-controller",
    "crates/tm-retrieval",
    "crates/tm-learning",
    "crates/tm-procedural",
    "crates/tm-reasoning",
    "crates/tm-mcp",
    "crates/tm-ipc",
    "crates/tm-app",
    "crates/tm-cli",
]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
uuid = { version = "1", features = ["v7", "serde"] }
thiserror = "2"
tracing = "0.1"
tracing-subscriber = "0.3"
```

**Dependency graph (simplified):**

```
tm-app ─┬─ tm-controller ──┬── tm-graph
        │                   ├── tm-vector
        │                   ├── tm-cluster
        │                   └── tm-trace
        ├─ tm-retrieval ────┤
        ├─ tm-capture       │
        ├─ tm-canon ────────┤
        ├─ tm-governance    │
        ├─ tm-reasoning     │
        ├─ tm-learning      │
        ├─ tm-procedural ───┤  (depends on tm-graph)
        ├─ tm-mcp           │
        ├─ tm-ipc           │
        └─ tm-embedding     │
                            │
                    tm-core ┘  (shared by all)
```

---

## 5. Embedding Pipeline

### 5.1 Model Selection

| Model | Dimensions | Size (ONNX) | Latency (CPU, single) | Quality (MTEB avg) |
|-------|-----------|-------------|----------------------|-------------------|
| all-MiniLM-L6-v2 | 384 | 80MB | ~25ms | 0.630 |
| all-MiniLM-L12-v2 | 384 | 120MB | ~45ms | 0.649 |
| bge-small-en-v1.5 | 384 | 130MB | ~35ms | 0.640 |

**Decision:** all-MiniLM-L6-v2. Acceptable quality, smallest size, fastest inference. Fits within install budget.

### 5.2 Model Loading

```rust
pub struct EmbeddingEngine {
    session: ort::Session,
    tokenizer: tokenizers::Tokenizer,
    max_seq_len: usize,          // 256 tokens (model max)
    embedding_dim: usize,        // 384
}

impl EmbeddingEngine {
    pub fn new(model_dir: &Path) -> Result<Self> {
        let env = ort::Environment::builder()
            .with_name("tracemind")
            .with_execution_providers([
                ort::ExecutionProvider::CPU(Default::default()),
            ])
            .build()?;

        let session = ort::Session::builder()?
            .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
            .with_intra_threads(2)?          // limit CPU usage
            .with_inter_threads(1)?
            .with_memory_pattern(true)?      // optimize memory allocation
            .commit_from_file(model_dir.join("model.onnx"))?;

        let tokenizer = tokenizers::Tokenizer::from_file(
            model_dir.join("tokenizer.json")
        )?;

        Ok(Self { session, tokenizer, max_seq_len: 256, embedding_dim: 384 })
    }
}
```

### 5.3 Inference

```rust
impl EmbeddingEngine {
    /// Embed a single text. Returns normalized 384-dim vector.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self.tokenizer.encode(text, true)?;
        let input_ids: Vec<i64> = encoding.get_ids()
            .iter()
            .take(self.max_seq_len)
            .map(|&id| id as i64)
            .collect();
        let attention_mask: Vec<i64> = encoding.get_attention_mask()
            .iter()
            .take(self.max_seq_len)
            .map(|&m| m as i64)
            .collect();
        let token_type_ids: Vec<i64> = vec![0i64; input_ids.len()];

        let seq_len = input_ids.len();

        let outputs = self.session.run(ort::inputs! {
            "input_ids" => ndarray::Array2::from_shape_vec(
                (1, seq_len), input_ids
            )?,
            "attention_mask" => ndarray::Array2::from_shape_vec(
                (1, seq_len), attention_mask.clone()
            )?,
            "token_type_ids" => ndarray::Array2::from_shape_vec(
                (1, seq_len), token_type_ids
            )?,
        }?)?;

        // Mean pooling over token embeddings, masked by attention
        let token_embeddings = outputs[0].try_extract_tensor::<f32>()?;
        let view = token_embeddings.view();  // shape: (1, seq_len, 384)

        let mut pooled = vec![0.0f32; self.embedding_dim];
        let mut total_weight = 0.0f32;

        for t in 0..seq_len {
            let mask = attention_mask[t] as f32;
            total_weight += mask;
            for d in 0..self.embedding_dim {
                pooled[d] += view[[0, t, d]] * mask;
            }
        }

        // Normalize
        for d in 0..self.embedding_dim {
            pooled[d] /= total_weight.max(1e-9);
        }
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        for d in 0..self.embedding_dim {
            pooled[d] /= norm.max(1e-9);
        }

        Ok(pooled)
    }

    /// Batch embed. Groups texts to minimize padding waste.
    pub fn embed_batch(&self, texts: &[&str], batch_size: usize) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size.min(32)) {
            // Sort by length within chunk to minimize padding
            let mut indexed: Vec<(usize, &str)> = chunk.iter()
                .enumerate()
                .map(|(i, t)| (i, *t))
                .collect();
            indexed.sort_by_key(|(_, t)| t.len());

            for (_, text) in &indexed {
                results.push(self.embed(text)?);
            }
        }
        Ok(results)
    }
}
```

### 5.4 Quantization

For memory-constrained environments, apply dynamic INT8 quantization at model load time:

```rust
// ONNX Runtime dynamic quantization (applied once at first load)
pub fn quantize_model(input_path: &Path, output_path: &Path) -> Result<()> {
    // Uses onnxruntime-extensions or pre-quantized model
    // Reduces model from 80MB -> ~22MB with <2% quality loss
    // Inference latency: ~18ms (faster due to INT8 ops)
    ...
}
```

**Model file management:**

| File | Size | Purpose |
|------|------|---------|
| model.onnx | 80MB | FP32 model (default) |
| model_q8.onnx | 22MB | INT8 quantized (optional, user config) |
| tokenizer.json | 700KB | HuggingFace tokenizer |
| special_tokens_map.json | 1KB | Special token config |

---

## 6. Memory Controller FSM

### 6.1 State Diagram

```
                    +-----------+
                    |   IDLE    |<---------------------------+
                    +-----+-----+                            |
                          |                                  |
                    CaptureEvent received                    |
                          |                                  |
                          v                                  |
                  +-----------------+                        |
                  |   CANONICALIZING |                       |
                  +--------+--------+                       |
                           |                                |
                     canonicalization complete               |
                           |                                |
                           v                                |
                  +-----------------+                        |
              +---|   GOVERNING     |                       |
              |   +--------+--------+                       |
              |            |                                |
         fact dropped   fact allowed                        |
              |            |                                |
              v            v                                |
        +--------+  +----------------+                      |
        | LOGGED |  | GATE_DECIDING  |                      |
        | (audit)|  +-------+--------+                      |
        +--------+          |                               |
                      +-----+-----+                         |
                      |           |                         |
                   STORE       DEFER                        |
                      |           |                         |
                      v           v                         |
              +-------------+  +--------+                   |
              |  STORING    |  | DEFERRED|                  |
              +------+------+  | (logged)|                  |
                     |         +--------+                   |
                     v                                      |
              +-------------+                               |
              |  INDEXING   |  (vector + cluster update)    |
              +------+------+                               |
                     |                                      |
                     v                                      |
              +-------------+                               |
              |  STORED     |  (trace emitted)              |
              +------+------+                               |
                     |                                      |
                     +--------------------------------------+
```

**On retrieval request (separate path):**

```
                    +-----------+
                    |   IDLE    |
                    +-----+-----+
                          |
                    RetrievalRequest received
                          |
                          v
                  +-----------------+
                  | ARM_SELECTING   |  (UCB bandit picks arm)
                  +--------+--------+
                           |
                           v
                  +-----------------+
                  | RETRIEVING      |  (3-phase pipeline)
                  +--------+--------+
                           |
                           v
                  +-----------------+
                  | RANKING         |  (re-rank, budget trim)
                  +--------+--------+
                           |
                           v
                  +-----------------+
                  | TRACE_RECORDING |  (log trace with arm name)
                  +--------+--------+
                           |
                           v
                  +-----------------+
                  | RESPONSE_READY  |  -> caller
                  +-----------------+
```

### 6.2 FSM Implementation

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerState {
    Idle,
    Canonicalizing,
    Governing,
    GateDeciding,
    Storing,
    Indexing,
    Stored,
    Deferred,
    ArmSelecting,
    Retrieving,
    Ranking,
    TraceRecording,
    ResponseReady,
}

pub struct MemoryControllerFSM {
    state: ControllerState,
    transition_log: Vec<(ControllerState, ControllerState, i64)>, // (from, to, timestamp)
}

impl MemoryControllerFSM {
    pub fn transition(&mut self, event: ControllerEvent) -> Result<ControllerState> {
        let next = match (self.state, &event) {
            (Idle, ControllerEvent::CaptureReceived(_)) => Canonicalizing,
            (Canonicalizing, ControllerEvent::CanonComplete(_)) => Governing,
            (Governing, ControllerEvent::FactAllowed(_)) => GateDeciding,
            (Governing, ControllerEvent::FactDropped(_)) => Idle,
            (GateDeciding, ControllerEvent::StoreDecision) => Storing,
            (GateDeciding, ControllerEvent::DeferDecision) => Deferred,
            (Storing, ControllerEvent::StoreComplete) => Indexing,
            (Indexing, ControllerEvent::IndexComplete) => Stored,
            (Stored, ControllerEvent::Ack) => Idle,
            (Deferred, ControllerEvent::Ack) => Idle,

            // Retrieval path
            (Idle, ControllerEvent::RetrievalRequest(_)) => ArmSelecting,
            (ArmSelecting, ControllerEvent::ArmSelected(_)) => Retrieving,
            (Retrieving, ControllerEvent::RetrievalComplete(_)) => Ranking,
            (Ranking, ControllerEvent::RankingComplete(_)) => TraceRecording,
            (TraceRecording, ControllerEvent::TraceRecorded) => ResponseReady,
            (ResponseReady, ControllerEvent::Ack) => Idle,

            (from, event) => {
                return Err(TraceMindError::InvalidTransition {
                    from,
                    event: format!("{:?}", event),
                });
            }
        };

        self.transition_log.push((self.state, next, now_millis()));
        self.state = next;
        Ok(next)
    }
}
```

### 6.3 Store/Defer Gate Logic

```rust
pub struct StoreGate {
    confidence_threshold: f64,    // default: 0.5
    max_entities_per_event: usize, // default: 20
    dedup_window_ms: i64,         // default: 5000
    recent_hashes: LruCache<u64, i64>, // content hash -> timestamp
}

impl StoreGate {
    pub fn decide(&mut self, fact: &CanonicalFact) -> GateDecision {
        // 1. Dedup check
        let hash = seahash::hash(fact.content_key().as_bytes());
        if let Some(ts) = self.recent_hashes.get(&hash) {
            if now_millis() - ts < self.dedup_window_ms {
                return GateDecision::Defer("duplicate within window");
            }
        }

        // 2. Confidence check
        if fact.confidence < self.confidence_threshold {
            return GateDecision::Defer("below confidence threshold");
        }

        // 3. Rate limit check
        if fact.entities.len() > self.max_entities_per_event {
            return GateDecision::Defer("entity count exceeds limit");
        }

        self.recent_hashes.put(hash, now_millis());
        GateDecision::Store
    }
}
```

### 6.4 Confidence Decay

```rust
/// Exponential decay: confidence(t) = confidence_0 * exp(-lambda * age_hours)
/// Default lambda = ln(2) / ttl_hours (half-life = ttl_hours)
pub fn decay_confidence(original: f64, age_hours: f64, ttl_hours: f64) -> f64 {
    let lambda = (2.0_f64).ln() / ttl_hours;
    let decayed = original * (-lambda * age_hours).exp();
    decayed.max(0.0)
}

/// Applied in learning loop. Entities below threshold after decay are candidates for forgetting.
pub fn apply_decay_sweep(graph: &mut impl GraphBackend, threshold: f64) -> usize {
    let now = now_millis();
    let mut decayed_count = 0;

    for entity in graph.all_entities_with_ttl() {
        let age_hours = (now - entity.updated_at) as f64 / 3_600_000.0;
        if let Some(ttl) = entity.ttl_hours {
            let new_conf = decay_confidence(entity.confidence, age_hours, ttl);
            if new_conf < threshold {
                graph.mark_for_forget(entity.id);
                decayed_count += 1;
            } else if (new_conf - entity.confidence).abs() > 0.01 {
                graph.update_confidence(entity.id, new_conf);
            }
        }
    }
    decayed_count
}
```

---

## 7. Retrieval Algorithm

### 7.1 Three-Phase Retrieval Pipeline

```
Input: query (String), budget (usize = 50), arm (BanditArm)
Output: RankedResults { facts: Vec<ScoredFact>, trace: RetrievalTrace }

Phase 1: Vector Search (LanceDB)
  - Embed query -> q_vec (384-dim)
  - ANN search: top-k = min(arm.breadth * 2, budget)
  - Result: Vec<(memory_id, cosine_similarity)>
  - Budget consumed: |results|

Phase 2: Graph Traversal (Kuzu/SQLite)
  - Seed entities: extract entity IDs from Phase 1 results (via source_id links)
  - Add entities: fuzzy name match on query keywords (>3 chars)
  - BFS expansion: depth = arm.depth, max_neighbors = arm.breadth per node
  - Dedup: skip already-seen entity IDs
  - Budget remaining: budget - |Phase 1 results|
  - Truncate to budget remaining

Phase 3: Cluster Expansion
  - For each seed entity from Phase 1+2, look up cluster_id
  - For each unique cluster, retrieve top-k members by membership_score
  - Filter: only include members NOT already in result set
  - Budget remaining: budget - |Phase 1| - |Phase 2|
  - Truncate to budget remaining

Phase 4: Procedural Recall
  - After cluster expansion, check all retrieved entity IDs for HAS_PROCEDURE edges
  - For each linked Procedure with confidence > procedure_threshold (default 0.5):
    - Include procedure name, trigger, and steps in retrieval context
  - Filter: only Active or Reinforced procedures (skip Degraded/Deprecated/Revised)
  - Procedure results are appended to facts but do NOT consume the fact budget
    (procedures are structural context, not individual facts)
```

### 7.2 Pseudocode

```rust
pub fn retrieve(
    &self,
    query: &str,
    arm: &BanditArm,
    budget: usize,
) -> Result<RetrievalResult> {
    let timer = Instant::now();
    let mut used_budget = 0usize;
    let mut seen_ids: HashSet<Uuid> = HashSet::new();
    let mut all_facts: Vec<ScoredFact> = Vec::new();

    // ── Phase 1: Vector Search ──────────────────────────────
    let q_vec = self.embedding.embed(query)?;
    let vector_k = (arm.breadth * 2).min(budget);
    let vector_hits = self.vector_store.search(&q_vec, vector_k)?;

    for hit in &vector_hits {
        if seen_ids.insert(hit.id) {
            all_facts.push(ScoredFact {
                id: hit.id,
                score: hit.similarity,
                source: RetrievalPhase::Vector,
                content: hit.text.clone(),
            });
            used_budget += 1;
        }
    }

    // ── Phase 2: Graph Traversal ────────────────────────────
    let graph_budget = budget.saturating_sub(used_budget);
    if graph_budget > 0 {
        // Seed from vector hits
        let mut seed_entities: Vec<Uuid> = vector_hits.iter()
            .filter_map(|h| h.source_entity_id)
            .collect();

        // Add keyword-matched entities
        let keywords: Vec<&str> = query.split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();
        for kw in &keywords {
            let matches = self.graph.fuzzy_search(kw, 5)?;
            seed_entities.extend(matches);
        }

        seed_entities.dedup();
        seed_entities.truncate(arm.breadth);

        // BFS expansion
        let mut graph_facts = Vec::new();
        for entity_id in &seed_entities {
            if used_budget + graph_facts.len() >= budget { break; }
            let neighbors = self.graph.get_neighbors(
                *entity_id,
                arm.depth,
                arm.breadth,
            )?;
            for (triple, score) in neighbors {
                if seen_ids.insert(triple.id) {
                    graph_facts.push(ScoredFact {
                        id: triple.id,
                        score: score * 0.8,  // slight discount vs vector
                        source: RetrievalPhase::Graph,
                        content: format!("{} {} {}",
                            triple.subject, triple.predicate, triple.object),
                    });
                }
            }
        }
        graph_facts.truncate(graph_budget);
        used_budget += graph_facts.len();
        all_facts.extend(graph_facts);
    }

    // ── Phase 3: Cluster Expansion ──────────────────────────
    let cluster_budget = budget.saturating_sub(used_budget);
    if cluster_budget > 0 {
        let cluster_ids: HashSet<i32> = seen_ids.iter()
            .filter_map(|id| self.cluster_store.get_cluster(*id).ok())
            .flatten()
            .collect();

        let mut cluster_facts = Vec::new();
        for cid in cluster_ids {
            if cluster_facts.len() >= cluster_budget { break; }
            let members = self.cluster_store.top_members(cid, 5)?;
            for member in members {
                if seen_ids.insert(member.entity_id) {
                    let entity = self.graph.get_entity(member.entity_id)?;
                    cluster_facts.push(ScoredFact {
                        id: member.entity_id,
                        score: member.membership_score * 0.6,  // cluster discount
                        source: RetrievalPhase::Cluster,
                        content: entity.description.unwrap_or_default(),
                    });
                }
            }
        }
        cluster_facts.truncate(cluster_budget);
        all_facts.extend(cluster_facts);
    }

    // ── Phase 4: Procedural Recall ─────────────────────────
    let mut procedures: Vec<ProcedureContext> = Vec::new();
    for fact_id in seen_ids.iter() {
        if let Ok(procs) = self.graph.get_procedures_for_entity(*fact_id) {
            for proc in procs {
                if proc.confidence > self.procedure_threshold
                    && matches!(proc.status, ProcedureStatus::Active | ProcedureStatus::Reinforced)
                {
                    procedures.push(ProcedureContext {
                        id: proc.id,
                        name: proc.name.clone(),
                        trigger: proc.trigger.clone(),
                        steps: proc.steps.clone(),
                        confidence: proc.confidence,
                    });
                }
            }
        }
    }
    procedures.dedup_by_key(|p| p.id);

    // ── Re-rank ─────────────────────────────────────────────
    all_facts.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    all_facts.truncate(budget);

    let trace = RetrievalTrace {
        query: query.to_string(),
        arm_name: arm.name.clone(),
        phases: [
            vector_hits.len(),
            used_budget - vector_hits.len(),
            all_facts.len().saturating_sub(used_budget),
        ],
        total_facts: all_facts.len(),
        latency_ms: timer.elapsed().as_millis() as u64,
    };

    Ok(RetrievalResult { facts: all_facts, procedures, trace })
}
```

### 7.3 Budget Allocation Strategy

| Arm Name | Vector Budget % | Graph Budget % | Cluster Budget % | Total Budget |
|----------|---------------|---------------|-----------------|--------------|
| narrow | 60% | 30% | 10% | 15 facts |
| medium | 50% | 35% | 15% | 25 facts |
| wide | 40% | 40% | 20% | 40 facts |
| deep | 30% | 50% | 20% | 50 facts |

---

## 8. Learning Pipeline

### 8.1 UCB Bandit Update

The controller maintains four arms (narrow, medium, wide, deep). Arm selection uses UCB1:

```
UCB1(arm_i) = Q(arm_i) + c * sqrt(ln(N) / n_i)

Where:
  Q(arm_i) = total_reward_i / n_i          (average reward)
  N = sum of all arm pulls
  n_i = number of times arm_i was pulled
  c = sqrt(2) ~= 1.414                     (exploration constant)
```

**Reward signal computation:**

```rust
pub fn compute_reward(trace: &ContextTrace) -> f64 {
    let mut reward = 0.0;

    // Base reward from outcome
    reward += match trace.outcome {
        Outcome::Success => 1.0,
        Outcome::Failure => -0.5,
        Outcome::Unknown => 0.0,
    };

    // Human feedback (strongest signal, overrides base)
    if let Some(feedback) = &trace.feedback {
        reward = feedback.score;  // [-1.0, 1.0], direct override
    }

    // Efficiency bonus: fewer facts used = better (if outcome was good)
    if reward > 0.0 {
        let fact_count = trace.retrieved_memory_ids.len() as f64;
        let efficiency = 1.0 - (fact_count / 50.0).min(1.0); // 50 = max budget
        reward += efficiency * 0.2;  // up to +0.2 bonus
    }

    // Latency penalty
    if trace.duration_ms > 100 {
        reward -= 0.1;
    }

    reward.clamp(-1.0, 1.5)
}
```

**Bandit update step:**

```rust
impl BanditArm {
    pub fn update(&mut self, reward: f64) {
        self.pulls += 1;
        self.total_reward += reward;
    }
}

impl AgentMemController {
    pub fn select_arm(&self) -> &BanditArm {
        // If any arm has 0 pulls, select it (forced exploration)
        if let Some(arm) = self.arms.iter().find(|a| a.pulls == 0) {
            return arm;
        }

        // UCB1 selection
        let total_pulls: u64 = self.arms.iter().map(|a| a.pulls).sum();
        self.arms.iter()
            .max_by(|a, b| {
                let ucb_a = a.q_value() + (2.0 * (total_pulls as f64).ln() / a.pulls as f64).sqrt();
                let ucb_b = b.q_value() + (2.0 * (total_pulls as f64).ln() / b.pulls as f64).sqrt();
                ucb_a.partial_cmp(&ucb_b).unwrap()
            })
            .unwrap()
    }

    pub fn register_reward(&mut self, reward: f64, arm_name: &str) {
        if let Some(arm) = self.arms.iter_mut().find(|a| a.name == arm_name) {
            arm.update(reward);
        }
    }
}
```

### 8.2 Trajectory Recording

Every controller interaction (store or retrieval) records a trajectory step:

```rust
pub struct TrajectoryRecorder {
    current_session: String,
    steps: Vec<TrajectoryStep>,
    writer: ParquetWriter,
}

impl TrajectoryRecorder {
    pub fn record_step(
        &mut self,
        state: &StateVector,
        action: MemoryAction,
        params: &HashMap<String, f64>,
        reward: f64,
        next_state: &StateVector,
    ) {
        self.steps.push(TrajectoryStep {
            step_index: self.steps.len() as u32,
            state: state.clone(),
            action,
            action_params: params.clone(),
            reward,
            next_state: next_state.clone(),
            timestamp: now_millis(),
        });
    }

    pub fn flush_trajectory(&mut self) -> Result<()> {
        if self.steps.is_empty() { return Ok(()); }

        let total_reward: f64 = self.steps.iter().map(|s| s.reward).sum();
        let trajectory = Trajectory {
            trajectory_id: Uuid::now_v7(),
            session_id: self.current_session.clone(),
            steps: std::mem::take(&mut self.steps),
            total_reward,
            created_at: now_millis(),
        };

        self.writer.write_trajectory(&trajectory)?;
        Ok(())
    }
}
```

### 8.3 Lightweight Policy Network (Phase 2)

After accumulating sufficient trajectories (>500), train a small MLP to replace/augment the UCB bandit:

```
Network Architecture:
  Input:  StateVector (393 features)
  Hidden: Linear(393, 128) -> ReLU -> Dropout(0.1)
  Hidden: Linear(128, 64) -> ReLU -> Dropout(0.1)
  Output: Linear(64, 4) -> Softmax  (probability over 4 arms)

Total parameters: 393*128 + 128 + 128*64 + 64 + 64*4 + 4 = 58,884
Model size: ~230KB (f32 weights)

Training:
  - Algorithm: REINFORCE with baseline
  - Baseline: exponential moving average of returns (alpha = 0.99)
  - Learning rate: 1e-4
  - Batch size: 32 trajectories
  - Update frequency: idle-time only, max once per hour
  - Training budget: max 10 seconds wall-clock per update
```

**Policy gradient update:**

```
For each trajectory T = [(s_0, a_0, r_0), ..., (s_n, a_n, r_n)]:
  G_t = sum_{k=t}^{n} gamma^{k-t} * r_k     (discounted return, gamma = 0.99)
  advantage_t = G_t - baseline

  loss = -sum_t [ log(pi(a_t | s_t)) * advantage_t ]

  baseline <- alpha * baseline + (1 - alpha) * mean(G)
```

**Fallback:** If policy net produces degenerate outputs (entropy < 0.1 or single arm > 0.95 probability), revert to UCB bandit for that query.

### 8.4 RL-Based Memory CRUD (Phase 3, Future)

Memory-R1 style: treat memory CRUD as an MDP.

```
State:   (query_embedding, graph_snapshot_embedding, action_history)
Actions: { Store(entity, triple), Update(entity_id, field, value),
           Delete(entity_id), Link(entity_a, entity_b, predicate), NoOp }
Reward:  downstream task success + human feedback
Training: Offline GRPO over trajectory batches

This is Phase 3 and will not be implemented in MVP. The trajectory storage
format in Phase 1/2 is designed to support this future training.
```

### 8b. JEPA/World Model/SSM Training Pipeline

All models in this section are Phase 3 and train locally during idle time. No cloud dependencies.

#### 8b.1 JEPA Training (Self-Supervised Representation Learning)

Learns latent representations of memory context by predicting target embeddings from context embeddings, without reconstructing raw inputs.

```
Architecture:
  Context Encoder:  MLP (384 -> 256 -> 384), ReLU activations
  Target Encoder:   EMA copy of Context Encoder (tau = 0.996, updated per batch)
  Predictor:        MLP (256 -> 384), bridges context -> target latent space

Loss: VICReg (Variance-Invariance-Covariance Regularization)
  L = lambda * invariance_loss + mu * variance_loss + nu * covariance_loss
  lambda = 25.0, mu = 25.0, nu = 1.0

Training Data:   Stored trajectories (context_embedding pairs from consecutive steps)
Batch Size:      64
Learning Rate:   3e-4 (AdamW, weight_decay = 1e-4)
Schedule:        Cosine annealing over idle training window
Parameters:      ~5M total
Model Size:      ~20MB on disk (f32 weights)
Training Cadence: During idle (no user queries for >5 min), max 30 min per session
```

#### 8b.2 World Model Training (Outcome Prediction)

Learns to predict outcomes from (state, action) pairs. Used for intent prediction, procedure simulation (dry-run), and surprise-based ingestion gating.

```
Architecture:
  Input:   state_dim (384) + action_dim (one-hot 5 actions + params ~ 32) = 416
  Hidden:  Linear(416, 256) -> ReLU -> Linear(256, 256) -> ReLU
  Output:  Linear(256, 384)  (predicted outcome embedding)

Loss: MSE(predicted_outcome, actual_outcome_embedding)
  + 0.1 * cosine_embedding_loss (directional alignment)

Training Data:   Trajectories where actual_outcome_embedding is non-null
Batch Size:      32
Learning Rate:   1e-4 (AdamW)
Parameters:      ~2-5M total
Model Size:      ~10-20MB on disk
Training Cadence: Co-trained with JEPA during idle windows

Applications:
  - Intent prediction: given current state, predict most likely next outcome
  - Procedure dry-run: simulate procedure steps through WM before live execution
  - Surprise scoring: surprise = ||WM_predicted - actual_outcome||
```

#### 8b.3 SSM (Mamba) Integration (Compressed Episodic History)

State Space Model that compresses full episodic trace history into a fixed-size hidden state, replacing linear episodic scans at inference time.

```
Architecture:
  Model:           Mamba block (selective state space model)
  Input:           Trajectory embeddings (384-dim per step)
  Hidden State:    Fixed-size (512-dim), carries compressed history
  Output:          Context vector for retrieval augmentation

Parameters:        ~1-3M total
Model Size:        ~5-12MB on disk
Inference:         Single forward pass through hidden state (replaces linear scan)
Training Cadence:  Extended idle only (overnight), requires >1000 trajectories

Benefit: O(1) inference for episodic context vs O(n) scan over trace history.
         Hidden state updated incrementally as new trajectories arrive.
```

#### 8b.4 Surprise-Based Ingestion Gate

Replaces the static confidence gate (section 6.3) with a learned surprise signal from the World Model.

```
Algorithm:
  1. On new CaptureEvent, compute context_embedding via MiniLM
  2. Feed (current_state, predicted_action) into World Model
  3. Get predicted_outcome embedding
  4. After actual outcome observed, compute:
       surprise = ||predicted_outcome - actual_outcome_embedding||_2
  5. Decision:
       if surprise > high_threshold (default 0.7):  STORE (novel information)
       if surprise < low_threshold  (default 0.2):  DEFER (redundant/expected)
       else:                                         fall back to confidence gate

Thresholds auto-calibrated from trajectory surprise_score distribution
(target: store top 30% by surprise, defer bottom 30%).
```

#### 8b.5 Total Model Budget

| Component | Parameters | Disk Size | RAM (loaded) | Phase |
|-----------|-----------|-----------|--------------|-------|
| JEPA (context + target + predictor) | ~5M | ~20MB | ~20MB | 3 |
| World Model (MLP) | ~2-5M | ~10-20MB | ~10-20MB | 3 |
| SSM / Mamba | ~1-3M | ~5-12MB | ~5-12MB | 3 |
| **Total** | **~8-13M** | **~40-60MB** | **~40-60MB** | 3 |

All models trainable on CPU (no GPU required). Training uses ONNX Runtime or `candle` for Rust-native inference. Models serialized as safetensors for fast loading.

---

## 9. IPC and Communication

### 9.1 Architecture Overview

```
+------------------+          +-------------------+
|  Tauri Frontend  |  <--->   |  Rust Backend     |
|  (SolidJS/Svelte)|  Tauri   |  (tm-app)         |
|  (webview)       |  invoke  |                   |
+------------------+          +---------+---------+
                                        |
                    +-------------------+-------------------+
                    |                   |                   |
              +-----+------+    +------+------+    +------+------+
              | MCP Server |    | Browser Ext |    | CLI (tmind) |
              | (stdio)    |    | (IPC socket)|    | (IPC socket)|
              +------------+    +-------------+    +-------------+
```

### 9.2 Tauri Frontend <-> Rust Backend

Tauri v2 provides a direct invoke mechanism. Frontend calls Rust functions via `invoke()`. No HTTP overhead.

**Command registration:**

```rust
// tm-app/src/commands.rs
#[tauri::command]
async fn memory_search(
    state: tauri::State<'_, AppState>,
    query: String,
    max_results: Option<usize>,
) -> Result<Vec<MemoryResult>, String> {
    let results = state.retrieval_engine
        .retrieve(&query, max_results.unwrap_or(20))
        .await
        .map_err(|e| e.to_string())?;
    Ok(results)
}

#[tauri::command]
async fn memory_store(
    state: tauri::State<'_, AppState>,
    text: String,
    source: String,
) -> Result<StoreResult, String> {
    let event = CaptureEvent::manual(text, source);
    state.pipeline.process(event).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_graph_neighborhood(
    state: tauri::State<'_, AppState>,
    entity_id: String,
    depth: Option<u32>,
) -> Result<GraphNeighborhood, String> { ... }

#[tauri::command]
async fn get_traces(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<TraceSnapshot>, String> { ... }

#[tauri::command]
async fn submit_feedback(
    state: tauri::State<'_, AppState>,
    trace_id: String,
    score: f64,
    correction: Option<String>,
) -> Result<(), String> { ... }
```

**Frontend call example (SolidJS):**

```typescript
import { invoke } from "@tauri-apps/api/core";

const results = await invoke<MemoryResult[]>("memory_search", {
  query: "authentication module decisions",
  maxResults: 20,
});
```

### 9.3 IPC Socket Protocol (Browser Extension, CLI)

External clients connect via Unix domain socket (macOS/Linux) or named pipe (Windows).

**Socket path:** `~/.tracemind/tracemind.sock` (Unix) or `\\.\pipe\tracemind` (Windows)

**Wire protocol:** Length-prefixed Cap'n Proto messages.

```
+--------+-------------------+
| 4 bytes| N bytes           |
| (N, LE)| Cap'n Proto msg   |
+--------+-------------------+
```

**Cap'n Proto schema:**

```capnp
@0xb1a2c3d4e5f60001;

struct Request {
  id @0 :UInt64;
  method @1 :Text;       # "search", "store", "feedback", "status"
  payload @2 :Data;      # JSON-encoded method-specific params
}

struct Response {
  id @0 :UInt64;         # matches request id
  status @1 :UInt16;     # 200 = ok, 400 = bad request, 500 = error
  payload @2 :Data;      # JSON-encoded result
  error @3 :Text;        # error message if status != 200
}
```

**Why Cap'n Proto over JSON:** Zero-copy deserialization. The 4-byte length prefix + Cap'n Proto body avoids JSON parse overhead (~3x faster for typical messages). JSON is used only inside `payload` fields for flexibility.

### 9.4 MCP Server (Claude Code)

The MCP server communicates over stdin/stdout using JSON-RPC 2.0 (MCP protocol standard).

**Lifecycle:**

```
Claude Code                          TraceMind MCP Server
    |                                       |
    |-- initialize -->                      |
    |                 <-- capabilities --    |
    |-- tools/list -->                      |
    |                 <-- tool list --       |
    |-- tools/call { memory_search } -->    |
    |                 <-- result --          |
    |-- tools/call { memory_store } -->     |
    |                 <-- result --          |
    ...
    |-- shutdown -->                         |
```

**MCP server startup:** Registered in Claude Code's MCP config (`~/.claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "tracemind": {
      "command": "tracemind-mcp",
      "args": [],
      "env": {
        "TRACEMIND_DATA": "~/.tracemind/data"
      }
    }
  }
}
```

### 9.5 Local REST API (Optional)

For third-party integrations. Runs on `127.0.0.1:9741` (port chosen to avoid conflicts). Bound to localhost only -- not accessible from network.

Built with Axum:

```rust
let app = Router::new()
    .route("/v1/search", post(handle_search))
    .route("/v1/store", post(handle_store))
    .route("/v1/entities/:id", get(handle_get_entity))
    .route("/v1/traces", get(handle_list_traces))
    .route("/v1/feedback", post(handle_feedback))
    .route("/v1/status", get(handle_status))
    .layer(
        tower_http::cors::CorsLayer::new()
            .allow_origin("http://localhost:*".parse().unwrap())
    );

axum::Server::bind(&"127.0.0.1:9741".parse().unwrap())
    .serve(app.into_make_service())
    .await?;
```

---

## 10. Browser Extension Architecture

### 10.1 Extension Structure (Chrome Manifest V3)

```
extensions/chrome/
  manifest.json
  background/
    service-worker.js       # Main background script
    ipc-client.js           # Native messaging / WebSocket to core
  content/
    capture.js              # Content script injected into pages
    selection.js            # Text selection capture
  popup/
    popup.html              # Extension popup UI
    popup.js                # Popup logic
  options/
    options.html            # Extension settings
    options.js
  icons/
    icon-16.png
    icon-48.png
    icon-128.png
```

**manifest.json (key fields):**

```json
{
  "manifest_version": 3,
  "name": "TraceMind Capture",
  "version": "1.0.0",
  "permissions": [
    "activeTab",
    "storage",
    "nativeMessaging"
  ],
  "host_permissions": ["<all_urls>"],
  "background": {
    "service_worker": "background/service-worker.js",
    "type": "module"
  },
  "content_scripts": [
    {
      "matches": ["<all_urls>"],
      "js": ["content/capture.js"],
      "run_at": "document_idle"
    }
  ],
  "externally_connectable": {
    "ids": ["*"]
  }
}
```

### 10.2 Capture Behavior

| Event | Data Captured | Trigger |
|-------|--------------|---------|
| Page visit | URL, title, timestamp | `navigation.onCompleted` |
| Text selection | Selected text, URL, surrounding context (100 chars) | `mouseup` in content script |
| Search query | Query text, search engine | URL pattern match (`google.com/search?q=`) |
| Tab switch | From/to URL, duration on previous tab | `tabs.onActivated` |
| Bookmark | URL, title, folder | `bookmarks.onCreated` |

**Content script (capture.js):**

```javascript
// Debounced selection capture
let selectionTimeout = null;
document.addEventListener("mouseup", () => {
    clearTimeout(selectionTimeout);
    selectionTimeout = setTimeout(() => {
        const selection = window.getSelection().toString().trim();
        if (selection.length > 10 && selection.length < 5000) {
            chrome.runtime.sendMessage({
                type: "text_selection",
                data: {
                    text: selection,
                    url: window.location.href,
                    title: document.title,
                    context: getSelectionContext(100),
                    timestamp: Date.now(),
                },
            });
        }
    }, 500);
});
```

### 10.3 Communication with Core

The browser extension communicates with the TraceMind core via **Native Messaging** (preferred) or fallback WebSocket to the local REST API.

**Native Messaging host manifest** (`com.tracemind.capture.json`):

```json
{
  "name": "com.tracemind.capture",
  "description": "TraceMind browser capture bridge",
  "path": "/usr/local/bin/tracemind-native-host",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://<extension-id>/"
  ]
}
```

The `tracemind-native-host` binary is a thin Rust program that reads Native Messaging frames from stdin and forwards them over the IPC socket to the core process.

**Message flow:**

```
Content Script -> Service Worker -> Native Host (stdin) -> IPC Socket -> Core
```

**Fallback (if native messaging unavailable):** Service worker connects via WebSocket to `ws://127.0.0.1:9741/ws/capture`. This is less reliable (requires REST server running) but works without native host installation.

### 10.4 Privacy Controls

- User-configurable blocklist of domains (stored in `chrome.storage.sync`)
- Incognito mode: extension disabled by default, user must explicitly enable
- All captures pass through governance funnel in core (PII filter applies)
- Extension popup shows capture status and allows pause/resume

---

## 11. Claude Code Integration

### 11.1 Hook Scripts

TraceMind registers Claude Code hooks that fire on specific lifecycle events.

**Hook installation** (`~/.claude/settings.json`):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "tracemind hook pre-tool-use"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "tracemind hook post-tool-use"
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "tracemind hook session-end"
          }
        ]
      }
    ]
  }
}
```

**Hook behavior:**

| Hook | Fires When | TraceMind Action |
|------|-----------|-----------------|
| `pre-tool-use` | Before any tool invocation | Log intent, check if memory has relevant context |
| `post-tool-use` | After tool returns | Capture tool result, extract entities/triples |
| `session-end` | Claude Code session ends | Flush trajectory, compute session reward |

**Hook script (`tracemind hook pre-tool-use`):**

Reads hook context from stdin (JSON), processes it, and optionally outputs to stdout:

```rust
// tm-cli/src/hooks.rs
pub fn handle_pre_tool_use(input: &HookInput) -> Result<HookOutput> {
    let tool_name = &input.tool_name;
    let tool_input = &input.tool_input;

    // 1. Record this as a trajectory step
    let state = build_state_vector(tool_name, tool_input)?;
    ipc_send("trajectory_step", &state)?;

    // 2. Check if we have relevant memory for this tool use
    let query = extract_query_from_tool_input(tool_name, tool_input);
    if let Some(query) = query {
        let memories = ipc_send("quick_search", &json!({
            "query": query,
            "max_results": 3,
            "timeout_ms": 50,  // strict latency budget for hooks
        }))?;

        if !memories.is_empty() {
            // Inject memory context as a note (does not block tool)
            return Ok(HookOutput {
                decision: "proceed",
                note: Some(format!(
                    "TraceMind context: {}",
                    format_memories(&memories)
                )),
            });
        }
    }

    Ok(HookOutput::proceed())
}
```

### 11.2 MCP Server Tools

The MCP server exposes these tools to Claude Code:

```json
{
  "tools": [
    {
      "name": "memory_search",
      "description": "Search TraceMind memory for relevant context. Returns structured facts with provenance.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "Natural language search query" },
          "max_results": { "type": "integer", "default": 10 },
          "include_graph": { "type": "boolean", "default": true },
          "include_procedures": { "type": "boolean", "default": true }
        },
        "required": ["query"]
      }
    },
    {
      "name": "memory_store",
      "description": "Store a new fact or decision in TraceMind memory.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "text": { "type": "string", "description": "The fact or decision to remember" },
          "entities": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Key entities mentioned"
          },
          "confidence": { "type": "number", "default": 0.9 },
          "source": { "type": "string", "default": "claude_code" }
        },
        "required": ["text"]
      }
    },
    {
      "name": "memory_feedback",
      "description": "Provide feedback on a previous memory retrieval.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "trace_id": { "type": "string" },
          "score": { "type": "number", "minimum": -1, "maximum": 1 },
          "correction": { "type": "string" }
        },
        "required": ["trace_id", "score"]
      }
    },
    {
      "name": "memory_graph",
      "description": "Explore the entity graph around a specific entity.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "entity_name": { "type": "string" },
          "depth": { "type": "integer", "default": 1, "maximum": 3 }
        },
        "required": ["entity_name"]
      }
    }
  ]
}
```

### 11.3 Memory Read/Write API

**Read (retrieval):**

```
Client -> MCP -> tools/call { name: "memory_search", arguments: { query: "auth module" } }
MCP    -> Core -> Retrieval Engine (3-phase)
Core   -> MCP <- { facts: [...], trace_id: "...", arm_used: "medium" }
MCP    -> Client <- { content: [{ type: "text", text: "..." }] }
```

**Write (storage):**

```
Client -> MCP -> tools/call { name: "memory_store", arguments: { text: "..." } }
MCP    -> Core -> Canonicalization -> Governance -> Store
Core   -> MCP <- { stored: true, entity_ids: [...], triple_ids: [...] }
MCP    -> Client <- { content: [{ type: "text", text: "Stored 3 entities, 2 triples." }] }
```

---

## 12. Storage Layout

### 12.1 File System Structure

```
~/.tracemind/
  config/
    settings.toml               # Global settings (embedding model path, ports, etc.)
    governance.toml              # PII patterns, custom rules
    consent.toml                 # Per-source consent flags
    rules.toml                   # User-defined governance rules

  data/
    graph/
      kuzu/                     # Kuzu database directory
        *.kz                    # Kuzu data files (~5MB base)
      fallback.sqlite           # SQLite fallback (only if Kuzu unavailable)

    vectors/
      memories.lance/           # LanceDB directory
        *.lance                 # Lance data files
        _indices/               # ANN index files
                                # ~20MB base + 1.5KB per 1000 vectors

    clusters/
      clusters.sqlite           # Cluster assignments + centroids (~1MB)

    traces/
      traces_20260328.parquet   # Daily trace files (~2MB/day)
      ...

    trajectories/
      traj_202603.parquet       # Monthly trajectory files (~5MB/month)
      ...

    audit/
      audit.parquet             # Governance audit log (append-only)

    models/
      policy_v001.bin           # Trained policy network weights (~230KB)

  logs/
    tracemind.log               # Application log (rotated, max 50MB)
    tracemind.log.1             # Previous rotation

  cache/
    embedding_cache.sqlite      # LRU embedding cache (text hash -> embedding)
                                # Max 10,000 entries, ~15MB

  tracemind.sock                # Unix domain socket (macOS/Linux)
  tracemind.pid                 # PID file for single-instance enforcement
```

### 12.2 Disk Usage Estimates

| Component | Base Size | Growth Rate | 1 Year Estimate |
|-----------|----------|-------------|-----------------|
| Kuzu graph | 5MB | ~500KB/month | 11MB |
| LanceDB vectors | 20MB | ~2MB/month | 44MB |
| Cluster store | 1MB | ~100KB/month | 2.2MB |
| Episodic traces | 0 | ~60MB/month | 720MB |
| Trajectories | 0 | ~5MB/month | 60MB |
| Audit log | 0 | ~10MB/month | 120MB |
| Embedding cache | 15MB | stable (LRU) | 15MB |
| Models | 80MB | stable | 80MB |
| Logs | 0 | stable (rotated) | 50MB |
| **Total** | **~121MB** | **~78MB/month** | **~1.1GB** |

**Compaction strategy:** Monthly compaction of traces and audit logs reduces storage by ~40% through Parquet column compression and dedup. With compaction, 1-year estimate drops to ~700MB.

### 12.3 Single-Instance Enforcement

```rust
pub fn acquire_lock(data_dir: &Path) -> Result<PidLock> {
    let pid_path = data_dir.join("tracemind.pid");

    // Check if another instance is running
    if pid_path.exists() {
        let existing_pid = std::fs::read_to_string(&pid_path)?;
        let pid: u32 = existing_pid.trim().parse()?;

        // Check if process is still alive
        if process_alive(pid) {
            return Err(TraceMindError::AlreadyRunning(pid));
        }
        // Stale PID file, remove it
        std::fs::remove_file(&pid_path)?;
    }

    // Write our PID
    std::fs::write(&pid_path, std::process::id().to_string())?;
    Ok(PidLock { path: pid_path })
}
```

---

## 13. Performance Budget

### 13.1 RAM Breakdown (Idle)

| Component | RSS (MB) | Notes |
|-----------|---------|-------|
| Rust runtime + static data | 8 | Binary BSS, stack, allocator |
| ONNX Runtime + model (loaded) | 95 | MiniLM-L6-v2 FP32 weights |
| Kuzu (mmap, cold) | 5 | Only header pages resident |
| LanceDB (mmap, cold) | 3 | Index metadata only |
| SQLite (cluster + cache) | 4 | Page cache, WAL |
| Tauri webview shell | 25 | Chromium-lite, idle |
| Tokio runtime | 3 | Thread pool (4 threads), channels |
| IPC listeners | 2 | Socket buffers |
| Miscellaneous (logs, config) | 5 | Tracing buffers, config structs |
| JEPA/WM/SSM models (Phase 3) | 0-40 | Loaded on-demand; 0 in Phase 1-2 |
| **Total Idle (Phase 1-2)** | **~150MB** | **Target: <200MB** |
| **Total Idle (Phase 3, models loaded)** | **~190MB** | **Target: <240MB** |

### 13.2 RAM Breakdown (Active Query)

| Component | Additional RSS (MB) | Notes |
|-----------|-------------------|-------|
| Query embedding inference | 15 | Temporary activations |
| LanceDB ANN search | 20 | Loaded index partitions |
| Kuzu graph traversal | 10 | Loaded pages for BFS |
| Result assembly | 5 | Serialized facts, traces |
| Frontend rendering | 30 | DOM, graph visualization |
| **Total Active** | **~230MB additional** | |
| **Total (Idle + Active)** | **~380MB** | **Target: <500MB** |

### 13.3 CPU Budget

| Task | CPU (% of single core) | Frequency | Duration |
|------|----------------------|-----------|----------|
| Idle monitoring (all captures) | <1% | Continuous | - |
| Single embedding inference | 100% of 2 threads | Per event | 25ms |
| Retrieval (3-phase) | 50% of 1 thread | Per query | 80ms |
| Governance pipeline | 5% of 1 thread | Per event | <2ms |
| Learning loop (bandit update) | 5% of 1 thread | Every 5min | <100ms |
| Learning loop (policy training) | 100% of 2 threads | Hourly, idle only | <10s |
| HDBSCAN incremental | 20% of 1 thread | Per 500 entities | <500ms |
| HDBSCAN full re-cluster | 100% of 2 threads | Weekly | <30s for 50k entities |
| JEPA + WM training (Phase 3) | 100% of 2 threads | Idle, max 30min/session | <30min |
| SSM/Mamba training (Phase 3) | 100% of 2 threads | Extended idle (overnight) | <2hr |
| Parquet compaction | 50% of 1 thread | Monthly | <60s |

**Idle detection:** Policy training and full re-clustering only run when system is idle (no user queries for >5 minutes AND system CPU <20%). Detected via `sysinfo` crate.

### 13.4 Disk I/O Limits

| Operation | Max IOPS | Max Throughput | Notes |
|-----------|---------|---------------|-------|
| Trace write | 10/s | 20KB/s | Buffered, flushed every 5s |
| Graph write | 50/s | 100KB/s | Kuzu WAL batching |
| Vector write | 20/s | 200KB/s | LanceDB append buffer |
| Compaction | 100/s burst | 10MB/s | Monthly, background |
| JEPA/WM model checkpoint | 5/s burst | 5MB/s | Per training session, ~60MB write |

---

## 14. Build and Distribution

### 14.1 Cross-Compilation Matrix

| Target | Triple | Toolchain | Notes |
|--------|--------|-----------|-------|
| macOS ARM | `aarch64-apple-darwin` | Xcode + Rust stable | Primary dev |
| macOS Intel | `x86_64-apple-darwin` | Xcode + Rust stable | Universal binary |
| Windows | `x86_64-pc-windows-msvc` | MSVC + Rust stable | |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | GCC + Rust stable | AppImage |
| Linux ARM | `aarch64-unknown-linux-gnu` | Cross-compile | RPi 5+ |

**macOS Universal Binary:**

```bash
# Build both architectures
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

# Combine with lipo
lipo -create \
  target/aarch64-apple-darwin/release/tracemind \
  target/x86_64-apple-darwin/release/tracemind \
  -output target/universal/tracemind
```

### 14.2 CI/CD Pipeline

```yaml
# .github/workflows/release.yml (conceptual)
stages:
  1. lint:
     - cargo fmt --check
     - cargo clippy -- -D warnings
     - cargo deny check licenses  # License compliance

  2. test:
     - cargo test --workspace
     - cargo test --workspace --features integration-tests
     - npm run test (frontend)

  3. build:
     parallel:
       - build-macos-arm64
       - build-macos-x86_64
       - build-windows-x86_64
       - build-linux-x86_64
       - build-linux-arm64

  4. package:
     - macOS: .dmg (universal binary) via create-dmg
     - Windows: .msi via WiX or NSIS
     - Linux: .AppImage + .deb + .rpm

  5. sign:
     - macOS: codesign + notarize (Apple Developer ID)
     - Windows: Authenticode signing
     - Linux: GPG signature on packages

  6. release:
     - GitHub Release with all artifacts
     - Update Homebrew tap (macOS)
     - Update winget manifest (Windows)
     - Update Flathub manifest (Linux)
```

### 14.3 Packaging Details

**macOS (.dmg):**
- Universal binary (ARM + Intel)
- Tauri bundles as `.app`
- Model files bundled inside `.app/Contents/Resources/models/`
- Native messaging host installed to `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`
- Estimated .dmg size: ~120MB (binary ~40MB + model ~80MB)

**Windows (.msi):**
- x86_64 only (ARM Windows not targeted initially)
- Tauri bundles as `.exe` with NSIS or WiX installer
- Model files in `%APPDATA%\tracemind\models\`
- Native messaging host registry key set during install
- Estimated .msi size: ~130MB

**Linux (.AppImage / .deb):**
- AppImage for universal distribution (self-contained)
- .deb for Debian/Ubuntu with apt repository
- Model files in `~/.local/share/tracemind/models/`
- Estimated AppImage size: ~140MB

### 14.4 Model Download Strategy

The ONNX model (80MB) is the largest artifact. Two strategies:

1. **Bundled (default):** Model included in installer. Larger download, zero post-install setup.
2. **Download-on-first-run:** Installer is 40MB. On first launch, downloads model from `https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2` (or cached CDN mirror). Progress shown in Tauri UI. SHA-256 verified.

Decision: **Bundled** for release builds (user expects it to work immediately). Download-on-first-run available as a `--lite` install option.

---

## 15. Testing Strategy

### 15.1 Test Pyramid

```
           /  E2E Tests  \              ~10 tests, <5min
          / (Tauri + real DBs) \
         /--------------------\
        / Integration Tests    \         ~50 tests, <2min
       / (cross-crate, real DBs) \
      /----------------------------\
     /     Unit Tests               \    ~500 tests, <30s
    / (per-crate, mocked deps)      \
   /----------------------------------\
```

### 15.2 Per-Crate Testing

| Crate | Unit Test Focus | Mock Strategy |
|-------|----------------|---------------|
| tm-core | Type serialization, validation | N/A (no deps) |
| tm-capture | Event parsing, dedup logic | Fake channel, mock OS APIs |
| tm-canon | Entity extraction accuracy, triple generation | Pre-built test corpus |
| tm-governance | PII detection recall/precision, rule evaluation | Test TOML configs |
| tm-graph | CRUD operations, query correctness | In-memory SQLite (fallback backend) |
| tm-vector | Insert/search accuracy, ANN recall | Temp LanceDB directory |
| tm-cluster | HDBSCAN correctness, incremental assignment | Synthetic embeddings |
| tm-embedding | Tokenization, pooling, normalization | Small test model (5MB) |
| tm-trace | Parquet write/read roundtrip, compaction | Temp directory |
| tm-controller | FSM transitions, bandit math, decay | Mock stores |
| tm-retrieval | 3-phase correctness, budget enforcement | Mock graph/vector/cluster |
| tm-learning | Reward computation, trajectory recording | Mock trace store |
| tm-mcp | JSON-RPC parsing, tool dispatch | Mock core |
| tm-ipc | Serialization roundtrip, connection handling | Loopback socket |

### 15.3 Integration Tests

```rust
#[cfg(test)]
mod integration {
    /// Full pipeline: capture -> canonicalize -> govern -> store -> retrieve
    #[tokio::test]
    async fn test_full_ingest_retrieve_cycle() {
        let env = TestEnv::new_temp().await;  // spins up all real stores in tmpdir

        // Ingest a fact
        let event = CaptureEvent::text("Rust uses LLVM for code generation", "test");
        env.pipeline.process(event).await.unwrap();

        // Wait for async indexing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Retrieve
        let results = env.retrieval.retrieve("what does Rust use for compilation?", 10).await.unwrap();
        assert!(!results.facts.is_empty());
        assert!(results.facts.iter().any(|f| f.content.contains("LLVM")));
    }

    /// Bandit convergence test: after N interactions with consistent feedback,
    /// the preferred arm should have highest Q-value.
    #[test]
    fn test_bandit_convergence() {
        let mut controller = AgentMemController::default();

        // Simulate 100 interactions where "wide" arm consistently gets better results
        for _ in 0..100 {
            let arm = controller.select_arm();
            let reward = match arm.name.as_str() {
                "wide" => 0.8,
                _ => 0.3,
            };
            controller.register_reward(reward, &arm.name);
        }

        // Wide arm should have highest Q-value
        let wide = controller.arms.iter().find(|a| a.name == "wide").unwrap();
        let best = controller.arms.iter().max_by(|a, b|
            a.q_value().partial_cmp(&b.q_value()).unwrap()
        ).unwrap();
        assert_eq!(best.name, "wide");
        assert!(wide.q_value() > 0.6);
    }

    /// Governance: PII should be filtered before reaching storage
    #[tokio::test]
    async fn test_pii_filtered() {
        let env = TestEnv::new_temp().await;

        let event = CaptureEvent::text(
            "Call John at 555-123-4567 or email john@secret.com",
            "test"
        );
        env.pipeline.process(event).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Search should not return raw PII
        let results = env.retrieval.retrieve("John contact info", 10).await.unwrap();
        for fact in &results.facts {
            assert!(!fact.content.contains("555-123-4567"));
            assert!(!fact.content.contains("john@secret.com"));
        }
    }
}
```

### 15.4 Performance Tests

```rust
#[cfg(test)]
mod bench {
    use criterion::{criterion_group, criterion_main, Criterion};

    fn bench_embedding_single(c: &mut Criterion) {
        let engine = EmbeddingEngine::new(test_model_path()).unwrap();
        c.bench_function("embed_single_sentence", |b| {
            b.iter(|| engine.embed("The quick brown fox jumps over the lazy dog"))
        });
        // Target: <30ms mean
    }

    fn bench_retrieval_full(c: &mut Criterion) {
        let env = BenchEnv::with_seeded_data(10_000);  // 10k entities, 25k triples
        c.bench_function("retrieval_3phase_10k", |b| {
            b.iter(|| env.retrieval.retrieve("authentication module", 50))
        });
        // Target: <100ms p95
    }

    fn bench_governance_pipeline(c: &mut Criterion) {
        let gov = GovernanceFunnel::from_config(test_governance_config());
        let fact = test_canonical_fact();
        c.bench_function("governance_check", |b| {
            b.iter(|| gov.evaluate(&fact))
        });
        // Target: <1ms mean
    }
}
```

### 15.5 Property-Based Tests

Using `proptest` for fuzzy inputs:

```rust
proptest! {
    #[test]
    fn entity_extraction_never_panics(input in "\\PC{0,10000}") {
        let extractor = EntityExtractor::default();
        let _ = extractor.extract(&input);  // must not panic
    }

    #[test]
    fn fsm_never_enters_invalid_state(events in prop::collection::vec(arb_event(), 0..100)) {
        let mut fsm = MemoryControllerFSM::new();
        for event in events {
            // Invalid transitions return Err, never corrupt state
            let _ = fsm.transition(event);
            // State must always be a valid variant
            assert!(matches!(fsm.state(), ControllerState::Idle | ... ));
        }
    }
}
```

---

## 16. Migration Plan

### 16.1 Current State (AgentMem v2)

The existing Python codebase uses:

| Component | Current Technology | Target Technology |
|-----------|-------------------|-------------------|
| Graph store | Neo4j (server process, JVM) | Kuzu (embedded, C++ FFI) |
| Vector store | ChromaDB (Python, SQLite+hnswlib) | LanceDB (Rust-native, columnar) |
| Clustering | HDBSCAN (scikit-learn, Python) | HDBSCAN (Rust port or C++ FFI) |
| Embeddings | OpenAI API or HuggingFace (Python) | ONNX Runtime (Rust, local) |
| Trace store | JSONL files | Parquet (arrow-rs) |
| Controller | Python (AgentMemController) | Rust (tm-controller) |
| Agent framework | AutoGen (Python) | Removed (TraceMind is not an agent) |
| IPC | None (monolithic) | Cap'n Proto over Unix socket |
| Frontend | None | Tauri + SolidJS/Svelte |

### 16.2 Migration Phases

**Phase M1: Data Export (Week 1)**

Export all data from Python/Neo4j into portable formats:

```python
# migration/export_neo4j.py
def export_entities(driver) -> List[Dict]:
    """Export all entities as JSON records."""
    with driver.session() as session:
        result = session.run(
            "MATCH (n:Entity) RETURN n.name AS name, n.type AS type, "
            "n.description AS description, n.confidence AS confidence, "
            "n.created_at AS created_at"
        )
        return [dict(record) for record in result]

def export_triples(driver) -> List[Dict]:
    """Export all relationships as JSON records."""
    with driver.session() as session:
        result = session.run(
            "MATCH (s:Entity)-[r:RELATED_TO]->(o:Entity) "
            "RETURN s.name AS subject, type(r) AS predicate, "
            "r.type AS rel_type, o.name AS object, "
            "r.confidence AS confidence, r.timestamp AS timestamp"
        )
        return [dict(record) for record in result]

# Output: entities.jsonl, triples.jsonl, traces.jsonl, clusters.json
```

**Phase M2: Rust Core Build (Weeks 2-6)**

Build the Rust crates incrementally:

| Week | Crates | Milestone |
|------|--------|-----------|
| 2 | tm-core, tm-embedding | Types compile, embeddings work |
| 3 | tm-graph, tm-vector | Can store and query entities/vectors |
| 4 | tm-canon, tm-governance | Ingest pipeline works end-to-end |
| 5 | tm-controller, tm-retrieval | FSM + 3-phase retrieval operational |
| 6 | tm-trace, tm-learning | Traces recorded, bandit updates |

**Phase M3: Data Import (Week 6)**

Import exported data into Rust stores:

```rust
// migration/src/import.rs
pub async fn import_from_jsonl(
    entities_path: &Path,
    triples_path: &Path,
    graph: &impl GraphBackend,
    vector: &VectorStore,
    embedding: &EmbeddingEngine,
) -> Result<ImportStats> {
    let mut stats = ImportStats::default();

    // Import entities
    let reader = BufReader::new(File::open(entities_path)?);
    for line in reader.lines() {
        let entity: ExportedEntity = serde_json::from_str(&line?)?;
        let embedding = embedding.embed(&entity.description.unwrap_or_default())?;

        graph.add_entity(&Entity {
            id: Uuid::now_v7(),
            name: entity.name,
            entity_type: parse_type(&entity.entity_type),
            description: entity.description,
            confidence: entity.confidence.unwrap_or(1.0),
            ..Default::default()
        })?;

        vector.insert(&entity.name, &embedding, /* metadata */)?;
        stats.entities += 1;
    }

    // Import triples (similar pattern)
    ...

    Ok(stats)
}
```

**Phase M4: Interface Build (Weeks 7-8)**

| Week | Component | Milestone |
|------|-----------|-----------|
| 7 | tm-mcp, tm-ipc, tm-cli | MCP server works with Claude Code |
| 8 | tm-app (Tauri), frontend | Desktop app with memory dashboard |

**Phase M5: Browser Extension + Polish (Weeks 9-10)**

| Week | Component | Milestone |
|------|-----------|-----------|
| 9 | Chrome extension | Capture works, native messaging operational |
| 10 | Integration testing, performance tuning | All targets met |

### 16.3 Validation Criteria

Migration is complete when:

1. All entities and triples from Neo4j are present in Kuzu (count match within 1%)
2. Vector search recall@10 on a held-out test set is within 5% of ChromaDB baseline
3. Retrieval latency p95 < 100ms (vs. ~500ms in Python/Neo4j)
4. RAM idle < 200MB (vs. ~800MB with Neo4j JVM)
5. Install size < 250MB (vs. ~2GB with Neo4j + Python + dependencies)
6. All existing test scenarios pass against new Rust backend
7. Claude Code MCP integration returns equivalent results for 50 test queries

### 16.4 Rollback Plan

During migration, the Python codebase remains functional. Both systems can run in parallel:

```
[Python/Neo4j] <-- reads/writes (production)
       |
       | (export script runs nightly)
       v
[Rust/Kuzu]    <-- reads only (shadow mode, compares results)
```

Shadow mode: Rust system processes the same queries as Python and logs result differences. When difference rate drops below 5% for 7 consecutive days, cutover to Rust.

---

## Appendix A: Key Dependencies and Versions

| Crate | Version | Purpose | License |
|-------|---------|---------|---------|
| tokio | 1.x | Async runtime | MIT |
| serde | 1.x | Serialization | MIT/Apache-2.0 |
| uuid | 1.x | UUID v7 generation | MIT/Apache-2.0 |
| ort | 2.x | ONNX Runtime bindings | MIT/Apache-2.0 |
| tokenizers | 0.20.x | HuggingFace tokenizer | Apache-2.0 |
| lancedb | 0.x | Vector database | Apache-2.0 |
| kuzu | 0.x | Graph database (FFI) | MIT |
| rusqlite | 0.32.x | SQLite bindings | MIT |
| arrow-rs | 53.x | Arrow/Parquet | Apache-2.0 |
| capnp | 0.20.x | Cap'n Proto | MIT |
| ndarray | 0.16.x | N-dimensional arrays | MIT/Apache-2.0 |
| tracing | 0.1.x | Structured logging | MIT |
| tauri | 2.x | Desktop framework | MIT/Apache-2.0 |
| clap | 4.x | CLI argument parsing | MIT/Apache-2.0 |
| notify | 7.x | File system watcher | MIT/Apache-2.0 |
| seahash | 4.x | Fast hashing | MIT |
| criterion | 0.5.x | Benchmarking | MIT/Apache-2.0 |
| proptest | 1.x | Property-based testing | MIT/Apache-2.0 |

## Appendix B: Configuration Reference

**`~/.tracemind/config/settings.toml`:**

```toml
[general]
data_dir = "~/.tracemind/data"
log_level = "info"                    # trace, debug, info, warn, error

[embedding]
model_path = "~/.tracemind/data/models"
model_name = "all-MiniLM-L6-v2"
quantized = false                     # true for INT8 quantized model
max_seq_len = 256
inference_threads = 2

[graph]
backend = "kuzu"                      # "kuzu" or "sqlite"
max_traversal_depth = 3
max_neighbors_per_hop = 20

[vector]
nprobes = 16
refine_factor = 2
reindex_threshold = 10000             # re-train index after this many inserts

[controller]
confidence_threshold = 0.5
ttl_hours_default = 720.0             # 30 days
decay_sweep_interval_s = 300
dedup_window_ms = 5000

[bandit]
exploration_constant = 1.414          # sqrt(2)
arms = ["narrow", "medium", "wide", "deep"]

[learning]
bandit_update_interval_s = 300
policy_training_interval_s = 3600
policy_training_max_duration_s = 10
min_trajectories_for_policy = 500

[retrieval]
default_budget = 30
max_budget = 50
timeout_ms = 100

[ipc]
socket_path = "~/.tracemind/tracemind.sock"    # Unix
pipe_name = "tracemind"                         # Windows
rest_port = 9741
rest_enabled = true

[capture]
clipboard_poll_interval_ms = 500
file_watcher_debounce_ms = 2000
channel_capacity = 1024

[governance]
pii_action = "redact"                 # "redact", "drop", "tag"
audit_enabled = true
```

## Appendix C: Error Taxonomy

```rust
#[derive(Debug, thiserror::Error)]
pub enum TraceMindError {
    // Storage errors
    #[error("Graph store error: {0}")]
    GraphError(String),
    #[error("Vector store error: {0}")]
    VectorError(String),
    #[error("Trace store error: {0}")]
    TraceError(String),

    // Pipeline errors
    #[error("Canonicalization failed: {0}")]
    CanonError(String),
    #[error("Governance rejected: rule={rule}, reason={reason}")]
    GovernanceRejection { rule: String, reason: String },

    // Controller errors
    #[error("Invalid FSM transition from {from:?} on event {event}")]
    InvalidTransition { from: ControllerState, event: String },

    // Embedding errors
    #[error("Embedding inference failed: {0}")]
    EmbeddingError(String),
    #[error("Model not found at {path}")]
    ModelNotFound { path: PathBuf },

    // IPC errors
    #[error("IPC connection failed: {0}")]
    IpcError(String),
    #[error("MCP protocol error: {0}")]
    McpError(String),

    // System errors
    #[error("Another TraceMind instance is running (PID {0})")]
    AlreadyRunning(u32),
    #[error("Configuration error: {0}")]
    ConfigError(String),

    // Generic
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
```
