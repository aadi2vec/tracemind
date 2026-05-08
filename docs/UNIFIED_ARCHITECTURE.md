# TraceMind — Unified Engine Architecture

**Status**: proposal — 2026-05-07
**Author**: Aaditya + Claude (co-founder brainstorm)
**Predecessors**: `BRAIN_ARCHITECTURE.md` (cognitive subsystem map), `INTENT_SYSTEM.md` (commitment primitive)
**Companion**: `PRODUCT_PORTFOLIO.md` (what we ship), `PHASE5_CONVERGENCE.md` (how we build it)

---

## 0. Purpose of this document

`BRAIN_ARCHITECTURE.md` describes the cognitive subsystem map (working
memory, episodic, semantic, etc.) and the brain-shaped internal
factoring. `INTENT_SYSTEM.md` describes the commitment primitive and
predictive layers. Both remain canonical for their scope.

This document describes **what changes when we add three new
capabilities** to the shared engine:

1. **Cross-modal reasoning** (idea 10) — reasoning chains that cross
   modalities (text, image, audio, code, structured data)
2. **Bitemporal belief management** (idea 12) — every fact carries
   valid-time and transaction-time; truth maintenance propagates
   retractions
3. **Code-intent semantics** (idea 22) — understanding code at the
   intent level, not the syntax level

These three capabilities serve all three products in the portfolio
(TraceMind, Engram, Rosetta) through the shared engine.

---

## 1. Engine crate map (24+ crates)

### Existing crates (20, unchanged in purpose)

```
binaries/integration ─ tm-cli, tm-mcp, tm-capture, tm-tauri
answer layer ──────── tm-answer
retrieval ─────────── tm-retrieval, tm-rerank, tm-reason, tm-controller
ingest ────────────── tm-ingest, tm-governance
intent ────────────── tm-intent, tm-reflect, tm-world-model
storage ───────────── tm-graph, tm-vector, tm-episodic
benchmarks ────────── tm-bench, tm-bench-locomo
core types ────────── tm-types
```

### New crates (4 shared, product-specific surfaces added later)

```
cross-modal ───────── tm-modal          (~2000 LOC)
bitemporal ────────── tm-temporal       (~1500 LOC)
truth maintenance ─── tm-tms            (~1500 LOC)
code semantics ────── tm-semcode        (~3000 LOC)
```

### Product-specific surfaces (added in later phases)

```
Engram SDK ────────── tm-engram         (Phase 2: wraps engine as belief API)
Rosetta CLI ───────── tm-rosetta        (Phase 3: wraps engine as code-intent tool)
```

### Full dependency graph

```
                 ┌─────────────────────────────────────────────────┐
                 │                 tm-types                         │
                 │  Entity, Triple, Trace, Trajectory, Commitment, │
                 │  Outcome, Anticipation, Need(NEW),              │
                 │  Sentiment(NEW), Action(NEW), ModalNode(NEW),   │
                 │  Belief trait(NEW), CodeIntent(NEW)             │
                 └──────────────────────┬──────────────────────────┘
                                        │
          ┌─────────────────────────────┼──────────────────────────────┐
          │                             │                              │
   ┌──────▼──────┐              ┌───────▼───────┐              ┌──────▼───────┐
   │  tm-modal   │              │  tm-temporal   │              │  tm-tms      │
   │  (NEW)      │              │  (NEW)         │              │  (NEW)       │
   │             │              │                │              │              │
   │  ModalNode  │              │  Bitemporal    │              │  JTMS        │
   │  encoders   │              │  valid_from/to │              │  Justificatn │
   │  projection │              │  txn_time      │              │  Dependency  │
   │  layers     │              │  time-travel   │              │  Propagation │
   │  co-occur   │              │  queries       │              │  Contradictn │
   │  edges      │              │                │              │  detection   │
   └──────┬──────┘              └───────┬────────┘              └──────┬───────┘
          │                             │                              │
          └─────────────────────────────┼──────────────────────────────┘
                                        │
   ┌────────────────────────────────────┼───────────────────────────────────┐
   │                                    │                                   │
   │                             ┌──────▼───────┐                          │
   │                             │  tm-graph    │                          │
   │                             │  (EXTENDED)  │                          │
   │                             │              │                          │
   │                             │  + bitemporal│                          │
   │                             │    columns   │                          │
   │                             │  + modal     │                          │
   │                             │    node type │                          │
   │                             │  + belief    │                          │
   │                             │    revision  │                          │
   │                             │    edges     │                          │
   │                             └──────┬───────┘                          │
   │                                    │                                  │
   ▼                                    ▼                                  ▼
┌──────────┐  ┌──────────┐  ┌──────────────┐  ┌──────────┐  ┌────────────┐
│tm-vector │  │tm-episodc│  │ tm-intent    │  │tm-reason │  │tm-semcode  │
│(EXTENDED)│  │          │  │ (EXTENDED)   │  │(EXTENDED)│  │  (NEW)     │
│          │  │          │  │              │  │          │  │            │
│+ multi-  │  │          │  │+ Belief      │  │+ cross-  │  │ tree-sitter│
│  modal   │  │          │  │  primitive   │  │  modal   │  │ AST parse  │
│  embed   │  │          │  │+ bitemporal  │  │  chains  │  │ intent     │
│  spaces  │  │          │  │  commitment  │  │+ belief  │  │  annotate  │
│          │  │          │  │  tracking    │  │  chains  │  │ semantic   │
│          │  │          │  │              │  │          │  │  diff      │
└──────────┘  └──────────┘  └──────────────┘  └──────────┘  └────────────┘
         │            │              │               │              │
         └────────────┴──────────────┴───────────────┘              │
                              │                                     │
                    ┌─────────▼──────────┐                         │
                    │   tm-retrieval     │                         │
                    │   (EXTENDED)       │                         │
                    │                    │                         │
                    │   + arm 5:         │                         │
                    │     cross-modal    │                         │
                    │   + bitemporal     │                         │
                    │     range filter   │                         │
                    │   + belief-aware   │                         │
                    │     ranking        │                         │
                    └─────────┬──────────┘                         │
                              │                                     │
              ┌───────────────┼───────────────────┐                │
              │               │                   │                │
       ┌──────▼──────┐ ┌─────▼──────┐ ┌──────────▼─────────────┐  │
       │  tm-cli     │ │  tm-mcp    │ │  tm-tauri              │  │
       │  tm-capture │ │  (EXTND)   │ │  (EXTENDED)            │  │
       └─────────────┘ └────────────┘ └────────────────────────┘  │
                                                                   │
                        ┌──────────────────────────────────────────┘
                        │
              ┌─────────▼──────────┐    ┌─────────────────────┐
              │  tm-engram (Ph.2)  │    │  tm-rosetta (Ph.3)  │
              │  Belief SDK API    │    │  Code-intent CLI    │
              │  MCP server        │    │  IDE extension      │
              └────────────────────┘    └─────────────────────┘
```

---

## 2. New crate: `tm-modal` — cross-modal reasoning

### Purpose

Make the nodes in reasoning chains modality-polymorphic. A node can be
text, a screenshot region, an audio segment, a code span, or structured
data. The chain builder doesn't care — it operates on embeddings + typed
edges.

### Types (added to `tm-types`)

```rust
/// A modality-tagged node in the knowledge graph. Extends Entity
/// with modality-specific metadata and a shared embedding projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalNode {
    pub id: Uuid,
    pub modality: Modality,
    pub source_ref: SourceRef,          // where this came from
    pub raw_embedding: Vec<f32>,        // modality-native embedding
    pub projected_embedding: Vec<f32>,  // projected to shared 384-dim space
    pub content_hash: u64,
    pub created_at: DateTime<Utc>,
    pub metadata: ModalMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modality {
    Text,
    Image,
    Audio,
    Code,
    Structured,   // JSON, CSV, API responses
}

/// Modality-specific metadata. The system doesn't need to understand
/// the internals of each modality — just enough to index and display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModalMetadata {
    Text { char_count: usize, language: Option<String> },
    Image { width: u32, height: u32, caption: String },
    Audio { duration_secs: f32, transcript: String },
    Code { language: String, file_path: String, span: (usize, usize) },
    Structured { schema_hint: String, row_count: Option<usize> },
}

/// How two modal nodes are connected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrossModalEdge {
    /// Temporal co-occurrence: captured within N seconds of each other.
    CoOccurrence { delta_ms: i64 },
    /// Containment: audio transcript contains text; screenshot depicts code.
    Contains,
    /// Reference: commit message references issue; PR links to screenshot.
    References { ref_type: String },
    /// Semantic similarity above threshold in shared space.
    SemanticLink { cosine: f32 },
    /// Causal: this screenshot was taken BECAUSE of this error log.
    Causal { direction: CausalDirection },
}
```

### Encoders

```rust
/// Trait for modality-specific encoders. Each encoder produces a
/// native embedding; tm-modal projects it to the shared 384-dim space.
pub trait ModalEncoder: Send + Sync {
    fn modality(&self) -> Modality;
    fn encode(&self, input: &[u8]) -> Result<Vec<f32>>;
    fn native_dim(&self) -> usize;
}
```

Implementations:

| Modality | Encoder | Model | Size | Native dim |
|---|---|---|---|---|
| Text | BGE-small-en-v1.5 (existing) | ONNX | ~30MB | 384 |
| Image | SigLIP-small | ONNX | ~100MB | 384 |
| Audio | Whisper-tiny features | ONNX | ~39MB | 384 |
| Code | tree-sitter AST + BGE | ONNX | ~30MB | 384 |
| Structured | Schema-aware BGE | ONNX | ~30MB | 384 |

Projection: per-modality learned linear layer (384×384) trained on
co-occurrence data. Ships with pretrained weights; fine-tunes on
user's own cross-modal co-occurrences over time.

### Integration with existing crates

- **`tm-ingest`**: new `ModalIngestPipeline` wraps existing text pipeline +
  adds image/audio/code/structured paths. Reuses governance gate.
- **`tm-graph`**: `ModalNode` stored as entities with `modality` property.
  `CrossModalEdge` stored as typed relations.
- **`tm-vector`**: extended to store multiple embedding spaces (one per
  modality + the shared projection). Index selection by modality.
- **`tm-reason::CrossModalChainBuilder`**: extends `ChainBuilder` to
  traverse cross-modal edges. Each hop in the chain carries its modality
  and evidence.
- **`tm-retrieval`**: new bandit arm (arm 5: cross-modal) fuses results
  across modalities. LinUCB context vector gets modality-mix features.
- **`tm-controller`**: learns when cross-modal retrieval helps vs. hurts.

### Co-occurrence edge generation

When two captures arrive within a configurable window (default: 30
seconds), `tm-modal` creates a `CoOccurrence` edge. This is the
cheapest, highest-signal way to link modalities — if you took a
screenshot and wrote a Slack message within 30 seconds, they're
probably about the same thing.

Heavier cross-modal linking (semantic similarity in shared space) runs
as a background consolidation pass in `tm-reflect`, not at ingest time.

---

## 3. New crate: `tm-temporal` — bitemporal data model

### Purpose

Every fact in the graph carries two time axes:

1. **Valid time** (`valid_from`, `valid_to`) — when the fact was true in
   the real world
2. **Transaction time** (`txn_at`) — when the system learned about it

This enables three classes of query that are currently impossible:

- **"What was true at time T?"** — valid-time query
- **"What did the system know at time T?"** — transaction-time query
- **"What did the system believe was true at time T, as known at time T'?"** — full bitemporal query

### Schema extensions to `tm-graph`

```sql
-- Entities gain temporal columns
ALTER TABLE entities ADD COLUMN valid_from TEXT;     -- ISO 8601
ALTER TABLE entities ADD COLUMN valid_to TEXT;       -- NULL = still valid
ALTER TABLE entities ADD COLUMN superseded_by TEXT;  -- UUID of replacement

-- Relations (triples) gain temporal columns
ALTER TABLE relations ADD COLUMN valid_from TEXT;
ALTER TABLE relations ADD COLUMN valid_to TEXT;
ALTER TABLE relations ADD COLUMN retracted_at TEXT;
ALTER TABLE relations ADD COLUMN retracted_by TEXT;  -- UUID of retracting event
ALTER TABLE relations ADD COLUMN retraction_reason TEXT;

-- Belief revisions: separate table for the TMS to consume
CREATE TABLE belief_revisions (
    id TEXT PRIMARY KEY,                -- UUID
    original_id TEXT NOT NULL,          -- entity or relation UUID
    original_type TEXT NOT NULL,        -- 'entity' or 'relation'
    revision_type TEXT NOT NULL,        -- 'retract' | 'supersede' | 'strengthen' | 'weaken'
    evidence TEXT,                      -- JSON array of trace UUIDs
    reason TEXT,
    txn_at TEXT NOT NULL,               -- when the revision was recorded
    valid_at TEXT                       -- when the revision took effect in reality
);
```

### Query API

```rust
/// Temporal query parameters. All fields optional — omit for
/// current-state queries (backwards-compatible).
pub struct TemporalQuery {
    /// Return facts valid at this point in real-world time.
    pub valid_at: Option<DateTime<Utc>>,
    /// Return facts as known by the system at this point.
    pub known_as_of: Option<DateTime<Utc>>,
    /// Include retracted facts in results (default: false).
    pub include_retracted: bool,
    /// Return the full revision history for matched facts.
    pub include_history: bool,
}

impl GraphStore {
    /// Time-travel query: returns the graph state at a specific point.
    pub fn query_at(&self, q: &TemporalQuery) -> Result<Vec<Entity>> { ... }

    /// Full history of a specific entity or relation.
    pub fn history_of(&self, id: Uuid) -> Result<Vec<BeliefRevision>> { ... }

    /// All facts that changed between two points in time.
    pub fn diff(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<TemporalDiff> { ... }
}
```

### Integration with `tm-intent`

Commitments already have `made_at` and state transitions. Bitemporal
extensions add:

- `Commitment.valid_from` = `made_at` (when the commitment became real)
- `Commitment.valid_to` = when it was completed/abandoned/superseded
- Every state transition is a `BeliefRevision` in the temporal store

This means `tm-intent` gets time-travel for free: "What commitments
were open on March 15?" is a valid-time query on the commitment table.

### Backwards compatibility

All temporal columns default to NULL. NULL `valid_from` = epoch.
NULL `valid_to` = still valid. Existing queries see current state
unchanged. Temporal queries are opt-in.

**Migration path for existing data**: a one-time migration backfills
`valid_from` from existing `created_at` timestamps and leaves `valid_to`
as NULL (still valid). Queries with `valid_at` parameter treat NULL
`valid_from` as epoch and NULL `valid_to` as still-valid, so
pre-migration records behave correctly without manual intervention.

---

## 4. New crate: `tm-tms` — Truth Maintenance System

### Purpose

Automatic propagation of belief revisions. When a foundation belief is
retracted, all beliefs that depend on it are flagged. Contradictions are
detected at assertion time, not in batch.

### Architecture

A Justification-based Truth Maintenance System (JTMS):

```rust
/// A justification: belief B is justified by beliefs A1, A2, ..., An.
/// If any Ai loses support, B's justification is invalidated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Justification {
    pub id: Uuid,
    pub belief_id: Uuid,              // the belief being justified
    pub supporting_beliefs: Vec<Uuid>, // beliefs that support this one
    pub justification_type: JustificationType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JustificationType {
    /// Direct evidence: "I believe X because I observed Y"
    Evidential,
    /// Deductive: "I believe X because Y and Y→X"
    Deductive,
    /// Inductive: "I believe X because Y1, Y2, Y3 all suggest X"
    Inductive,
    /// Default: "I believe X because nothing contradicts it"
    Default,
}

/// The TMS engine. Maintains the justification DAG and propagates
/// changes incrementally.
pub struct TruthMaintenanceSystem {
    justifications: HashMap<Uuid, Vec<Justification>>,
    dependents: HashMap<Uuid, Vec<Uuid>>,  // belief → beliefs that depend on it
    status: HashMap<Uuid, BeliefStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeliefStatus {
    /// Fully supported — all justifications hold.
    In,
    /// Support lost — at least one justification chain is broken.
    Out,
    /// Contradicted — this belief conflicts with another In belief.
    Contradicted,
}

impl TruthMaintenanceSystem {
    /// Assert a new belief with its justification. Returns any
    /// contradictions detected.
    pub fn assert_belief(
        &mut self,
        belief_id: Uuid,
        justification: Justification,
    ) -> Result<Vec<Contradiction>> { ... }

    /// Retract a belief. Propagates to all dependents.
    /// Returns all beliefs whose status changed.
    pub fn retract(
        &mut self,
        belief_id: Uuid,
        reason: &str,
    ) -> Result<Vec<StatusChange>> { ... }

    /// Find all current contradictions in the belief set.
    pub fn contradictions(&self) -> Vec<Contradiction> { ... }

    /// Explain why a belief has its current status.
    pub fn explain(&self, belief_id: Uuid) -> Result<Explanation> { ... }
}
```

### Contradiction detection

Two beliefs contradict when:

1. **Explicit negation** — belief A and belief "not A" both have status
   `In`. Detected by embedding similarity: if `cosine(embed_A, embed_B) < -0.8`
   and both are `In`, flag as contradiction. The -0.8 threshold is
   deliberately conservative (high precision, lower recall) — we'd
   rather miss a contradiction than flag a false positive. Threshold
   is tunable and will be calibrated against the TMS eval suite.

2. **Schema constraint** — a typed relation has a cardinality constraint
   (e.g., "person has exactly one birthday"). Two different values for the
   same constrained slot = contradiction.

3. **Temporal overlap** — two facts about the same entity with
   overlapping valid-time ranges that assert different values for the same
   property.

### Integration with existing crates

- **`tm-graph`**: the TMS reads justifications from graph edges (a triple
  "A supports B" is a justification). New edge types: `justifies`,
  `contradicts`.
- **`tm-intent`**: when a Commitment is `Superseded`, the TMS propagates
  to any Anticipations grounded in it.
- **`tm-reflect`**: the nightly consolidation pass runs TMS consistency
  checks. Contradictions surface in the daily brief.
- **`tm-reason`**: reasoning chains carry justification metadata. The TMS
  can explain why a chain's conclusion holds or doesn't.

### Persistence

TMS state is derived from the graph — it's a materialized view, not
independent state. On startup, the TMS rebuilds from graph edges with
`justifies` / `contradicts` types. Incremental updates during runtime;
full rebuild is cheap (linear in number of justification edges).

---

## 5. New crate: `tm-semcode` — code-intent semantics

### Purpose

Understand code at the intent level. Extract *what code is for*, not
just what it does syntactically. Enable intent-preserving transformations
and drift detection.

### Architecture

```rust
/// What a code unit is FOR — the semantic purpose extracted from
/// cross-modal evidence (code + tests + docs + commits + PRs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeIntent {
    pub id: Uuid,
    pub target: CodeTarget,           // what code unit this describes
    pub intent_statement: String,     // "validates payment amounts before charging"
    pub evidence: Vec<IntentEvidence>, // what grounded this extraction
    pub confidence: f32,
    pub extracted_at: DateTime<Utc>,
    pub embedding: Vec<f32>,          // 384-dim for similarity search
}

/// What code unit the intent describes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CodeTarget {
    Function { file: String, name: String, span: (usize, usize) },
    Module { path: String },
    Type { file: String, name: String },
    Test { file: String, name: String, tests_target: Box<CodeTarget> },
    File { path: String },
}

/// Evidence sources for intent extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentEvidence {
    /// The code itself (AST structure, naming, patterns)
    CodeAnalysis { language: String, ast_hash: u64 },
    /// Doc comments, README sections, inline comments
    Documentation { source: String, snippet: String },
    /// Test names and assertions describe expected behavior
    TestBehavior { test_names: Vec<String>, assertion_count: usize },
    /// Commit messages describe why changes were made
    CommitHistory { commits: Vec<CommitRef> },
    /// PR descriptions and review comments
    PrDiscussion { pr_id: String, relevant_snippets: Vec<String> },
}

/// A semantic diff: what changed in MEANING, not syntax.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticDiff {
    pub id: Uuid,
    pub from_commit: String,
    pub to_commit: String,
    pub changes: Vec<SemanticChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticChange {
    pub target: CodeTarget,
    pub change_type: SemanticChangeType,
    pub old_intent: Option<CodeIntent>,
    pub new_intent: Option<CodeIntent>,
    pub intent_preserved: bool,        // did the transformation preserve intent?
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SemanticChangeType {
    /// Mechanical: rename, move, format. Intent unchanged.
    Cosmetic,
    /// Behavioral: same intent, different implementation.
    Refactor,
    /// Intent changed: the code now does something different.
    IntentShift { description: String },
    /// Intent expanded: the code now does more than before.
    IntentExpansion { added: String },
    /// Intent narrowed: the code now does less than before.
    IntentReduction { removed: String },
    /// New code: no prior intent to compare against.
    New,
    /// Deleted code: intent removed entirely.
    Deleted,
}
```

### Intent extraction pipeline

```
Source code ──► tree-sitter parse ──► AST structure analysis
     │                                        │
     ▼                                        ▼
 Tests ──────► test name/assertion ──► behavioral evidence
     │            extraction                  │
     ▼                                        ▼
 Docs ───────► doc comment/README ──► purpose evidence
     │            extraction                  │
     ▼                                        ▼
 Git log ────► commit message ──────► historical evidence
     │            extraction                  │
     ▼                                        ▼
 PRs ────────► PR discussion ───────► review evidence
                                              │
                                              ▼
                                    Cross-modal fusion
                                    (tm-modal reasoning)
                                              │
                                              ▼
                                    CodeIntent extraction
                                    (confidence-weighted)
                                              │
                                              ▼
                                    Intent graph stored
                                    in tm-graph
```

### Integration with existing crates

- **`tm-graph`**: CodeIntent stored as entities. Intent relationships
  as edges: `function X implements intent Y`, `test T verifies intent Y`,
  `commit C changed intent Y`.
- **`tm-modal`**: code, tests, docs, commits, PRs are different modalities.
  Cross-modal reasoning chains connect them.
- **`tm-temporal`**: intent carries valid-time. "This function's intent
  was X from January to March, then shifted to Y." Git history provides
  the timeline.
- **`tm-tms`**: when a test is deleted, the TMS can flag that the intent
  it was verifying has lost evidential support.
- **`tm-reason`**: intent-aware reasoning chains. "Why does this function
  exist?" → chain through commits, PRs, and docs.

### Language support

Tree-sitter parsing for initial languages:
- Rust, Python, TypeScript/JavaScript, Go, Java

Each language needs a thin adapter (~200 LOC) that maps tree-sitter AST
node types to `CodeTarget` variants. The intent extraction pipeline
is language-agnostic above the adapter layer.

---

## 6. Extensions to existing crates

### `tm-graph` (extended)

New capabilities:
- Bitemporal columns on all entities and relations (§3)
- `ModalNode` storage alongside existing entities
- `CrossModalEdge` as typed relations
- `BeliefRevision` table for TMS consumption
- `CodeIntent` entities and intent-relationship edges
- New query methods: `query_at()`, `history_of()`, `diff()`

Implementation: all extensions are additive SQL columns and new tables.
No existing columns or tables change. Backwards-compatible.

### `tm-vector` (extended)

New capabilities:
- Multiple embedding spaces (one per modality + shared projection)
- Modality-filtered search: "find similar images" or "find similar code"
- Cross-modal search: "find anything similar" uses shared projection

Implementation: new index tables per modality. Existing text index
unchanged.

### `tm-intent` (extended) — the full intent arc

The intent primitive expands from `Commitment + Outcome` to the full
arc: `Need → Sentiment → Commitment → Action → Outcome`. New types
added to `tm-intent::types`:

```rust
/// The underlying driver behind an intent. Pre-commitment: exists as
/// soon as a gap between reality and desire is felt. Needs recur,
/// compound, and reveal what actually matters over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Need {
    pub id: Uuid,
    pub statement: String,             // "ship faster", "reduce vendor risk"
    pub urgency: f32,                  // 0..1, inferred or declared
    pub recurring: bool,               // does this need keep coming back?
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,      // updated each time need is re-expressed
    pub source: NeedSource,
    pub linked_commitments: Vec<Uuid>, // commitments spawned from this need
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedSource {
    /// Mined from phrases like "I need", "we have to", "the problem is"
    ImplicitMined,
    /// User explicitly stated via CLI/UI/MCP
    Declared,
    /// Inferred from recurring commitment patterns
    PatternInferred,
}

/// Emotional context attached to a need, commitment, or the general
/// topic area. Sentiment changes over time — tracked as a series of
/// updates rather than a single value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sentiment {
    pub id: Uuid,
    pub target_id: Uuid,               // Need, Commitment, or Entity this refers to
    pub target_type: SentimentTarget,
    pub valence: f32,                   // -1.0 (frustrated/anxious) to +1.0 (excited/confident)
    pub intensity: f32,                 // 0.0 (mild) to 1.0 (strong)
    pub source: SentimentSource,
    pub captured_at: DateTime<Utc>,
    pub evidence_text: Option<String>,  // the phrase that conveyed the sentiment
    pub evidence_trace: Option<Uuid>,   // pointer to the trace
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentimentTarget {
    Need,
    Commitment,
    Entity,
    Topic,       // general topic area, not a specific commitment
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentimentSource {
    /// Inferred from word choice and phrasing (heuristic or Tier-1 LLM)
    LanguageInferred,
    /// User explicitly stated ("I'm frustrated with X")
    Declared,
    /// Inferred from behavioral signals (re-asking, abandoning, etc.)
    BehavioralInferred,
}

/// An observed action taken in pursuit of (or response to) a
/// Commitment. Detected from capture streams — git commits, deployed
/// code, sent messages, completed tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: Uuid,
    pub commitment_id: Option<Uuid>,   // which commitment this action relates to
    pub description: String,
    pub taken_at: DateTime<Utc>,
    pub evidence: Vec<Uuid>,           // traces proving the action happened
    pub modality: ActionModality,
    pub source: ActionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionModality {
    GitCommit,
    Message,       // Slack, email, chat
    Deployment,
    FileChange,
    VoiceCapture,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSource {
    /// Detected from capture streams by embedding similarity to open commitments
    AutoDetected,
    /// User explicitly linked via CLI/UI/MCP
    Declared,
    /// Git integration detected the commit
    GitIntegration,
}
```

**Graph edges for the intent arc:**

```
Need ──spawns──► Commitment ──acted_on_by──► Action ──resulted_in──► Outcome
  │                  │
  └──felt_as──► Sentiment (updates over time)
                     │
                     └──felt_as──► Sentiment (updates over time)
```

**Additional capabilities:**
- `Belief` trait — a Rust trait that `Commitment` implements. `Belief`
  defines the core interface: `statement()`, `confidence()`,
  `valid_time_range()`, `justifications()`, `status()`. `Commitment`
  adds intent-specific fields (horizon, stakes, options, outcome).
  `Need`, `Sentiment`, and `Action` also implement `Belief` for Engram
  compatibility. Engram's API accepts any `impl Belief`.
- Bitemporal fields on all intent arc types (`valid_from`, `valid_to`)
- Every state transition logged as `BeliefRevision`
- `NeedDetector` in `tm-capture` — mines need-shaped phrases from
  existing capture streams
- `SentimentScorer` — heuristic valence scoring from word choice;
  Tier-1 LLM-assisted when available
- `ActionMatcher` — matches detected actions to open commitments by
  embedding similarity + temporal proximity

### `tm-reason` (extended)

New capabilities:
- `CrossModalChainBuilder` — extends `ChainBuilder` to traverse
  cross-modal edges. Each hop carries modality + evidence.
- `BeliefChainBuilder` — chains that track justification status.
  "This conclusion holds because beliefs A, B, C all have status In."
- `IntentChainBuilder` — chains through code intent relationships.
  "This function exists because of this PR which was motivated by
  this commit which references this issue."

### `tm-retrieval` (extended)

New capabilities:
- Bandit arm 5: cross-modal. Fuses results across modalities with
  modality-weighted RRA.
- Bitemporal range filter in the retrieval pipeline.
- Belief-aware ranking: boost results with `In` status, demote
  `Contradicted`, hide `Out` (unless user explicitly asks for
  retracted facts).
- LinUCB context vector extended with: modality-mix of query,
  temporal range width, belief status distribution of candidates.

### `tm-controller` (extended)

| Arm | Name      | top_k | hops | episodic | colbert | cross-modal |
|-----|-----------|-------|------|----------|---------|-------------|
| 0   | narrow    | 5     | 0    | no       | no      | no          |
| 1   | medium    | 10    | 1    | no       | no      | no          |
| 2   | wide      | 15    | 2    | no       | no      | no          |
| 3   | deep      | 20    | 2    | yes      | no      | no          |
| 4   | colbert   | 10    | 1    | no       | yes     | no          |
| 5   | xmodal    | 12    | 1    | no       | no      | yes         |

### `tm-reflect` (extended)

New capabilities in the nightly consolidation pass:
- TMS consistency check across all beliefs
- Cross-modal co-occurrence edge promotion (background, not real-time)
- Bitemporal garbage collection (archive retracted facts older than
  configurable retention period)
- Code-intent drift detection (compare current codebase intent graph
  against last snapshot)

### `tm-mcp` (extended)

New MCP tools:

| Tool | Purpose |
|---|---|
| `memory_need` | Record a need ("I need X") with urgency and tags |
| `memory_sentiment` | Record sentiment toward a need/commitment/topic |
| `memory_action` | Record an action taken toward a commitment |
| `memory_arc` | Query the full intent arc for a commitment (need → sentiment → commitment → actions → outcome) |
| `memory_believe` | Assert a belief with justification (Engram API) |
| `memory_retract` | Retract a belief with reason (Engram API) |
| `memory_world_at` | Time-travel query (bitemporal) |
| `memory_contradictions` | List current contradictions |
| `code_intent` | Extract intent for a code unit (Rosetta API) |
| `code_semantic_diff` | Semantic diff between two commits (Rosetta API) |
| `memory_cross_modal` | Cross-modal reasoning query |

---

## 7. Data layout (extended)

```
~/.tracemind/
├── memory.db                    # SQLite graph + vector (EXTENDED)
│   ├── entities (+ valid_from, valid_to, superseded_by, modality)
│   ├── relations (+ valid_from, valid_to, retracted_at, retraction_reason)
│   ├── needs (NEW — intent arc)
│   ├── sentiments (NEW — intent arc, append-only updates)
│   ├── actions (NEW — intent arc)
│   ├── belief_revisions (NEW)
│   ├── justifications (NEW)
│   ├── code_intents (NEW)
│   ├── modal_embeddings (NEW — per-modality index tables)
│   └── ... (existing tables unchanged)
├── memory.db-wal, memory.db-shm
├── traces.jsonl                 # immutable trace log (unchanged)
├── recent.jsonl                 # capture ring buffer (unchanged)
├── bandit.json                  # UCB1 state (+ arm 5)
├── linucb.json                  # contextual bandit (+ new features)
├── world_model/
│   ├── f_topic.bin              # next-topic MLP
│   └── f_outcome.bin            # outcome-polarity transformer
├── modal_projections/           # NEW: per-modality linear projections
│   ├── text_proj.bin
│   ├── image_proj.bin
│   ├── audio_proj.bin
│   ├── code_proj.bin
│   └── structured_proj.bin
└── models/
    ├── bge-384-v1.5/            # text embeddings (existing)
    ├── mxbai-colbert/           # rerank (existing)
    ├── siglip-small/            # NEW: image encoder
    ├── whisper-tiny/            # audio encoder (planned in Sprint B)
    └── gliner-*-multi/          # optional NER (existing)
```

---

## 8. Cross-product API surface

The shared engine exposes capabilities through three API layers:

### Layer 1: Rust crate API (all products)

Direct crate dependencies. TraceMind, Engram, and Rosetta all import
the engine crates directly. This is the highest-performance path.

### Layer 2: MCP server (TraceMind + Engram)

JSON-RPC 2.0 over stdio. Used by Claude Code, Cursor, and other MCP
clients. Engram's MCP tools (`memory_believe`, `memory_retract`,
`memory_world_at`, `memory_contradictions`) are the primary distribution
channel for agent developers.

### Layer 3: FFI bindings (future)

UniFFI for mobile (iOS/Android). C API for embedding in non-Rust
applications. Python/Node wrappers via PyO3/napi-rs for Engram SDK
distribution.

---

## 9. Design principles (carried forward + new)

**Carried forward from `BRAIN_ARCHITECTURE.md`:**
- Internal architecture is brain-shaped; external metaphor is open
- Local-only by default; network only for opt-in model downloads
- SQLite only for storage
- Embedding-space world model; never generates text about the user
- Every surface cites its sources
- Calibration-gated predictive layers (L1 silent / L2 opt-in / L3 gated)

**New principles for the unified engine:**
- **Temporal by default.** Every new fact gets timestamps. Omitting
  temporal metadata is the exception, not the rule.
- **Modality-agnostic reasoning.** Chain builders, retrievers, and
  rankers operate on the shared embedding space. Modality is metadata,
  not a constraint.
- **Belief-native, not document-native.** The unit of storage is a
  belief with justification, not a document chunk. Documents are evidence
  for beliefs.
- **Intent is the full arc.** Need → Sentiment → Commitment → Action →
  Outcome. The system captures the complete decision lifecycle, not just
  the commitment. Intent is what the system preserves and reasons about
  across form, modality, and time.
- **Additive extensions only.** No existing schema changes. All new
  capabilities are new columns, new tables, new types. Existing code
  paths work unchanged until they opt into the new capabilities.
