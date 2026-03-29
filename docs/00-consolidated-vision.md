# TraceMind: Consolidated Vision Document

**Author:** Aaditya Srivathsan
**Date:** 2026-03-28
**Status:** Living document — synthesized from Project Cloak, AgentMem v2, TraceMind v1/v2, and Technical Architecture docs

---

## 1. What TraceMind Is

TraceMind is a **local-only, privacy-first memory operating system for AI agents and humans**. It watches what you do (with your explicit permission), remembers what matters, learns what works, and makes your AI tools smarter over time — without any data ever leaving your machine.

Think of it as a **personal black box recorder for your digital life** that also happens to make every AI tool you use dramatically better.

---

## 2. The Convergence of Five Ideas

TraceMind synthesizes five separate design documents into one coherent product:

### 2.1 AgentMem v2 (Existing Codebase)
**Key contribution:** Structured memory with self-improvement
- Entity-centric knowledge graph (Neo4j) with typed, timestamped edges
- UCB bandit policy that learns which retrieval strategies work
- Episodic trace logs capturing full decision provenance
- Multi-agent reasoning via AutoGen GroupChat
- Confidence decay and controlled forgetting

### 2.2 Project Cloak
**Key contribution:** Local-only execution with governance
- CPU-native execution — no cloud, no GPU dependency
- FSM-based deterministic control flow (every decision replayable)
- JEPA-inspired latent learning for improved memory representations
- Memory Governance Module: policy filters, schema enforcement, access logs
- LLMs as non-authoritative utilities (perception, not decision)

### 2.3 Technical Architecture Documents
**Key contribution:** Enterprise-grade infrastructure patterns
- Python-first API with Rust performance core (PyO3 bindings)
- Policy-as-Code engine with runtime enforcement
- Cryptographic audit trails (Merkle-chain append-only logs)
- Actor-based async orchestration for concurrency
- Multi-tenant isolation, SOC2/GDPR compliance patterns
- Real-time rollback via system checkpoints

### 2.4 TraceMind v1
**Key contribution:** RL-based memory management
- Memory-R1: RL agent that learns ADD/UPDATE/DELETE/NOOP for memory CRUD
- Graph-R1: Multi-turn RL reasoning over hypergraph memory
- Agent-R1: Unified MDP framework for training memory and reasoning agents
- Trajectory storage for offline policy optimization
- Neural memory (Titans/MIRAS) for long-term generalization

### 2.5 TraceMind v2
**Key contribution:** Multimodal and domain-specific extensions
- Cross-modal canonicalization (text, images, charts, audio)
- Joint embedding space across modalities
- Domain-specific applications (finance, policy, compliance)
- Modality-tagged facts for cross-modal reasoning

---

## 3. The Cognitive Pipeline

TraceMind's memory is not a single store. It is a **five-layer cognitive pipeline** where each layer serves a distinct cognitive function, inspired by human memory systems and mapped to Google DeepMind's TITANS/MIRAS architecture.

| Layer | Type | Technology | Cognitive Role | Human Analogy | TITANS/MIRAS Equivalent |
|-------|------|-----------|---------------|---------------|------------------------|
| 1 | **Semantic** | Vector Store (LanceDB) | Recall "anything similar" via embeddings | **Intuition**: I've heard something like this before | Sliding Window Attention |
| 2 | **Structured** | Entity Graph (Kuzu/SQLite) | Connect facts via explicit typed relationships | **Knowledge**: I know X relates to Y because of Z | Long-Term Neural Memory (MLP) |
| 3 | **Clustered** | HDBSCAN Index | Group entities into higher-level themes | **Concepts**: This relates to "Finance" or "Security" | — |
| 4 | **Episodic** | Append-only Parquet | Record every interaction, decision, outcome | **Experience**: Last time I tried X, it worked/failed | — |
| 5 | **Procedural** | Graph + Vector hybrid | Store "how to do X" as versioned step sequences | **Skill**: I know how to deploy a service | Action Policy Head (Kinetic Memory) |

**How the pipeline flows:**
1. **Semantic Memory** provides seed entities (intuition match)
2. **Structured Memory** provides context around those seeds (relationship traversal)
3. **Clustered Memory** ensures theme-level coverage even without direct graph edges
4. **Episodic Memory** validates if the combination was useful, training the controller
5. **Procedural Memory** recalls *how to act* on what is known — the verbs, not just the nouns

### Procedural Memory: The Missing Piece

Most AI memory systems store facts. TraceMind also stores **skills** — versioned, executable procedures linked to entities:

```
Procedure {
    name: "deploy-to-staging"
    version: 3
    trigger: "user asks about deployment"
    steps: [
        { action: "run_tests", params: {...}, expected_outcome: "pass" },
        { action: "build_docker", params: {...}, expected_outcome: "image_built" },
        { action: "push_to_registry", params: {...}, expected_outcome: "pushed" }
    ]
    linked_entities: ["staging-server", "docker", "CI-pipeline"]
    confidence: 0.87
    status: Active | Reinforced | Degraded | Deprecated | Revised
}
```

Procedures have a lifecycle: they're created, reinforced by positive outcomes, degraded by failures, deprecated when stale, and revised into new versions. This mirrors how human skills evolve — you don't delete a skill, you refine it.

### Palantir Ontology Alignment

TraceMind's data model directly mirrors the Palantir Foundry ontology — the same model powering Palantir AIP:

| Palantir Concept | TraceMind Equivalent |
|---|---|
| **Objects** (Nouns) | Entity nodes in the Graph Store |
| **Links** (Relationships) | Triplet edges with confidence/timestamp |
| **Properties** | Node/edge metadata (confidence, version, source) |
| **Actions / Verbs** | Procedure nodes with executable steps |
| **Security Model** | Governance funnel (ACL + PII filter + audit) |
| **Schema Enforcement** | Type validation in governance layer |
| **Audit Trail** | ContextTrace + Merkle-chain log |

This alignment is deliberate: it means TraceMind's data model is enterprise-ready from day one, even when the initial user is a consumer.

---

## 4. The Unified Architecture

TraceMind collapses these five streams into a single layered architecture:

```
+------------------------------------------------------------------+
|                    CAPTURE LAYER (Passive Input)                  |
|  Browser Extension | Clipboard | Claude Code Hooks | File Watch  |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              CANONICALIZATION LAYER (Rust Native)                 |
|  Entity Extraction | Triple Generation | Embedding | Denoising   |
|  Multimodal encoding (text, images, links)                       |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              GOVERNANCE FUNNEL (Rust Native)                     |
|  Privacy Filters | Schema Validation | PII Detection             |
|  User-defined rules | Consent verification                      |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              STRUCTURED MEMORY LAYER (Embedded DBs)              |
|  Entity Graph (Kuzu/SQLite) | Vector Store (LanceDB)             |
|  Semantic Clusters (HDBSCAN) | Episodic Traces (Append-only)     |
|  Procedural Memory (Graph + Vector hybrid)                       |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              MEMORY CONTROLLER (FSM + Learned Policy)            |
|  Store/Defer Gate | Retrieval Policy (UCB Bandit)                |
|  Confidence Decay | Trajectory Recording                        |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              RETRIEVAL ENGINE                                    |
|  Phase 1: Vector Search (→ JEPA latent search in Phase 3)        |
|  Phase 2: Graph Traversal | Phase 3: Cluster Expansion           |
|  Phase 4: Procedural Recall | Bounded recall budget              |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              REASONING & INTENT LAYER                            |
|  Context continuation ("what you probably need next")            |
|  Pattern detection across sessions                               |
|  Proactive surfacing of relevant memories                        |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              OUTPUT INTERFACES                                   |
|  Desktop App (Tauri) | MCP Server | Claude Code Hooks            |
|  REST API (local only) | CLI                                     |
+------------------------------------------------------------------+
            |
            v
+------------------------------------------------------------------+
|              LEARNING LOOP (Background, Idle-time)               |
|  Phase 1: UCB Bandit updates from feedback                       |
|  Phase 2: Lightweight policy networks on trajectories            |
|  Phase 3: JEPA encoder + World Model + Surprise ingestion        |
|  Phase 4: SSM temporal compression + Memory-R1 CRUD agent        |
+------------------------------------------------------------------+
```

---

## 5. Latent Intelligence Roadmap (JEPA / World Models / SSMs)

The advanced roadmap moves TraceMind from symbolic memory toward **unified latent intelligence** — without becoming LLM-dependent. These are small, local models trained on YOUR trajectories.

### 5.1 JEPA — Latent Retrieval (Phase 3)

**Problem:** Vector cosine similarity matches surface text, not semantic intent. "How do I deploy?" and "What's our CI pipeline?" are semantically related but textually different.

**Solution:** A JEPA encoder (~5M params, MLP) predicts what a *good answer embedding* looks like in latent space, then retrieves memories matching that prediction. Trained on stored trajectories where we know which retrievals led to good outcomes.

**Where it fits:** Replaces the vector search phase of 3-phase retrieval. The bandit still selects depth/breadth, but vector phase uses JEPA-predicted targets instead of raw cosine.

**Constraint fit:** ~5M params = ~20MB. Trainable on CPU in ~10 minutes over stored trajectories.

### 5.2 World Models — Intent Prediction (Phase 3-4)

**Problem:** TraceMind surfaces relevant memories reactively. A truly useful assistant predicts what you need BEFORE you ask.

**Solution:** A world model (~2-5M params) trained on episodic traces (state, action, outcome) predicts: "Given your current context, you're probably about to need X." Also enables: "If you take action A, the likely outcome is B" — pre-execution simulation for procedures.

**Where it fits:** Powers the Reasoning & Intent Layer. Trained on the same trajectory data as JEPA. Also gates ProcedureExecutor: simulate before executing.

**Constraint fit:** ~2-5M params = ~10-20MB. Inference is a single forward pass.

### 5.3 SSMs (Mamba) — Temporal Compression (Phase 4)

**Problem:** Episodic traces grow linearly forever. After a year, scanning them is O(N) and RAM-prohibitive.

**Solution:** State Space Models compress temporal history into a **fixed-size hidden state**. Constant-time temporal queries regardless of history length. The hidden state implicitly learns what's worth remembering.

**Where it fits:** Replaces linear episodic store scans. Could eventually replace the UCB bandit with a learned temporal policy. Full trace log remains for audit/replay.

**Constraint fit:** Mamba inference is fast on CPU. Training needs careful batching during idle time.

### 5.4 Surprise-Based Ingestion

**Problem:** The current store/defer gate uses confidence thresholds (conf >= 0.4). This stores HIGH-CONFIDENCE facts, but for a consumer product you want to store NOVEL facts — things that are surprising.

**Solution:** Borrow TITANS' "surprise metric": measure how much a new fact deviates from the world model's prediction. High surprise = worth remembering. Low surprise = redundant, skip.

**Where it fits:** Replaces the static confidence threshold in the governance funnel once the world model exists. Until then, the confidence gate remains as fallback (Principle P6: Graceful Degradation).

### 5.5 The Unified Vision

```
    Query
      |
      v
  [JEPA Encoder] --- predicts latent target
      |
      v
  [Retriever] --- searches in latent space (not raw cosine)
      |
      v
  [World Model] --- simulates outcome before acting
      |         \
      v          v
  [Execute]   [Re-plan / Ask human]
      |
      v
  [ContextTrace] --- records full trajectory
      |
      v
  [SSM Temporal Memory] --- compresses episode into fixed state
      |         \
      v          v
  [Update JEPA]  [Update World Model]
```

### 5.6 Why This Isn't LLM-Dependent

| Component | Params | Training Data | Runs On |
|-----------|--------|--------------|---------|
| JEPA encoder | ~5M | Your trajectories | CPU, ~10min training |
| World model | ~2-5M | Your episodic traces | CPU, ~15min training |
| SSM (Mamba) | ~1-3M | Your temporal sequences | CPU, idle-time training |
| **Total** | **~8-13M** | **All local, all yours** | **No GPU needed** |

For reference, `all-MiniLM-L6-v2` is 22M params and runs fine on CPU. These models are smaller.

### 5.7 Training Data Strategy: Why Phase 1-2 Matter

JEPA, world models, and SSMs all need the same training data:

```
Trajectory = {
    state: (context_embedding, memory_snapshot_hash, user_activity_type),
    action: (retrieval_arm, memory_ids_retrieved, store_decisions),
    outcome: (user_feedback, task_success, correction_applied),
    predicted_outcome: (world_model_prediction — null until Phase 3),
    timestamp: ...,
    session_id: ...
}
```

**Phase 1-2 store these trajectories from day one.** By the time Phase 3 arrives, you have thousands of trajectories to train on — locally, on the user's own data. This is the critical design-now-build-later decision.

### 5.8 TITANS/MIRAS Gap Analysis

| TITANS Concept | TraceMind Status | Gap |
|---|---|---|
| Short-Term Memory (Sliding Window) | Phase 1 — context window in reasoning layer | None |
| Long-Term Memory (Neural MLP) | Phase 3 — JEPA encoder | Symbolic until Phase 3 |
| Kinetic Memory (Action Policy) | Phase 1 — Procedural Memory | Done (versioned procedures) |
| Memory Algorithm (Online Gradient) | Phase 3 — JEPA + WM training | Symbolic until Phase 3 |
| Surprise Metric (Loss Gradient) | Phase 3 — Surprise-based ingestion | Confidence gate until Phase 3 |
| Retention Gate (Regularizer) | Phase 1 — TTL decay + confidence gate | Done |

**The key gap:** differentiable memory update. Phases 1-2 are fully symbolic. Phase 3 introduces learned components (JEPA, WM) that update based on gradients — but only on the CONTROL plane (how memory is used), never on the DATA plane (what memory contains). This preserves Tenet #2.

---

## 6. Core Design Principles

These are non-negotiable across all layers:

### P1. Local-Only Execution
All computation — embedding, retrieval, clustering, reasoning, learning — happens on-device. No cloud calls. No telemetry. Data never leaves the machine. This is not a feature; it is the architecture.

### P2. Auditability Above Convenience
Every decision is reproducible from its trace. Every memory write has provenance. Every retrieval is logged with IDs and ordering. The FSM guarantees deterministic replay.

### P3. Lightweight by Design
Consumer laptops have 8-16GB RAM. TraceMind's memory footprint target: **<200MB resident at idle, <500MB under active use.** This means:
- Embedded databases only (no JVM, no server processes)
- Quantized embedding models (ONNX Runtime, <100MB)
- Memory-mapped indexes
- Lazy loading of historical data

### P4. Structure Controls Learning; Learning Controls Memory
Memory is never an unstructured blob. Facts are typed, timestamped, confidence-scored triples in a graph. Learning adjusts HOW memory is used (retrieval policy, store/defer decisions), never WHAT memory contains directly.

### P5. Human Corrections Are First-Class Memory
When a user corrects TraceMind, that correction goes through the full canonicalization and governance pipeline and becomes a permanent graph update — not a metadata annotation, not a log entry.

### P6. Graceful Degradation
Every advanced feature (clustering, learned policy, RL) has a rule-based fallback. The system works on day one with zero training data. It gets better with use.

### P7. Transparency IS the Product
Users can always see exactly what TraceMind knows, how it learned it, and why it surfaced something. The dashboard is not an afterthought — it is the primary trust mechanism.

---

## 5. What Makes TraceMind Different

### vs. Rewind/Limitless (screen recording + search)
Rewind records everything as video frames and OCR text. It's a **recording**. TraceMind builds **structured understanding** — entity graphs, semantic clusters, causal chains. You can ask TraceMind "why did I decide X?" and get a traced answer. You can't do that with a screen recording.

### vs. mem0/Zep/LangMem (cloud AI memory)
These are cloud-first memory APIs for developers. Your data goes to their servers. TraceMind is local-only by architecture, not by policy. There is no server to send data to. Additionally, these tools are just vector stores with metadata — TraceMind maintains a typed knowledge graph with governance.

### vs. Apple Intelligence (on-device AI)
Apple's approach is opaque — you can't see what it learned or why. TraceMind's entire value proposition is transparency: full audit trail, deterministic replay, visible memory graph. Apple also doesn't learn from outcomes or improve retrieval over time.

### vs. Microsoft Recall (activity recording)
Recall was killed by privacy backlash because it was a black box that recorded everything. TraceMind's governance funnel means users control exactly what gets stored, with visible policies and PII filtering. The trust model is inverted: opt-in granular capture, not opt-out surveillance.

### vs. RAG / Vector-only systems
RAG systems stuff context into prompts with no structure. When retrieval is wrong, there's no way to know why. TraceMind's multi-phase retrieval (vector + graph + cluster) is bounded, auditable, and improvable. Every retrieved fact has a trace ID.

---

## 6. The Claude Code Integration Story

### Today: Claude Code is Stateless
- No memory between sessions
- Rediscovers your codebase every time
- Can't learn from past mistakes
- No audit trail of AI-assisted decisions

### With TraceMind: Claude Code Becomes a Learning Collaborator

**As a Hook (passive capture):**
- Every Claude Code session is recorded as an episodic trace
- File changes, decisions, reasoning steps → structured memory
- User corrections become graph updates
- Over time, TraceMind knows: "In this codebase, the user prefers X pattern over Y"

**As an MCP Server (active memory):**
- Claude Code queries TraceMind: "What did we decide about the auth module?"
- TraceMind returns structured context with provenance
- Retrieval is bounded and auditable (not "stuff everything into context")
- Claude Code can write new memories: "User confirmed this approach works"

**The compound effect:**
Session 1: Claude Code is generic
Session 10: Claude Code knows your naming conventions
Session 50: Claude Code knows which approaches succeeded/failed in your codebase
Session 200: Claude Code has a deep, structured model of your project's architecture, decisions, and rationale — all queryable, all auditable, all local.

---

## 7. The Consumer "Wow Moments"

### Moment 1: "It remembered."
You researched something in your browser 2 weeks ago. You're now in Claude Code working on a related problem. TraceMind surfaces that research automatically — the articles you read, the notes you took, the decisions you made.

### Moment 2: "Show me why."
You ask TraceMind: "Why did I choose PostgreSQL for this project?" It shows you the full trace: the articles you read, the comparisons you made, the conversation where you decided, and the outcome (project succeeded/failed).

### Moment 3: "It learned."
Early on, TraceMind retrieves too much irrelevant context. After 50 interactions with feedback, its bandit policy has converged — retrievals are tighter, more relevant, and measurably better. You can see this improvement in the dashboard.

### Moment 4: "My AI knows me."
Claude Code, powered by TraceMind, doesn't ask "what framework are you using?" for the hundredth time. It knows. It knows your preferred patterns, your past decisions, your team's conventions. It's not AI with amnesia anymore.

---

## 8. Technical Stack (MVP)

| Layer | Technology | Why |
|-------|-----------|-----|
| Data/Control Plane | Rust | Performance, memory safety, no GC pauses, <50MB binary |
| Desktop App Shell | Tauri | Rust backend + web frontend, ~10MB install, cross-platform |
| Entity Graph | Kuzu (embedded) or SQLite | No server process, <5MB footprint, ACID |
| Vector Store | LanceDB (embedded, Rust) | Columnar, fast, embedded, ~20MB |
| Clustering | HDBSCAN (Rust port) | CPU-efficient, incremental updates |
| Embeddings | ONNX Runtime + all-MiniLM-L6-v2 | 80MB model, fast CPU inference |
| Trace Store | Append-only Parquet files | Columnar, compressed, fast scans |
| Browser Extension | Chrome Extension (Manifest V3) | Captures URLs, selections, search queries |
| Claude Code Integration | MCP Server (Rust) + Hook scripts | Bidirectional memory access |
| Learning | UCB Bandit → Lightweight policy net | Minimal CPU, runs during idle |
| Frontend | SolidJS or Svelte | Tiny bundle, reactive, fast |
| Serialization | Cap'n Proto or FlatBuffers | Zero-copy IPC between Rust services |

**Total install size target:** <250MB (including embedding model)
**RAM at idle:** <150MB
**RAM under active use:** <400MB

---

## 9. Phased Roadmap

### Phase 1: Foundation (Weeks 1-4) — "It remembers"
- Rust core: 5-layer memory engine (semantic + structured + clustered + episodic + procedural)
- Canonicalization pipeline (entity extraction, triple generation, procedure detection)
- Governance funnel (PII filter, schema validation, user rules, confidence gate)
- UCB bandit retrieval controller with 4-phase recall (vector + graph + cluster + procedural)
- Tauri desktop app with Cognitive Pipeline visualization
- Chrome extension for passive capture
- Claude Code MCP server + hooks
- **Trajectory storage from day one** (designed for JEPA/WM/SSM training in Phase 3)

### Phase 2: Learning (Weeks 5-8) — "It gets smarter"
- Feedback loop: user ratings → bandit policy updates
- Procedure lifecycle: Active → Reinforced → Degraded → Deprecated → Revised
- Lightweight policy network trained on stored trajectories (idle-time)
- Context continuation engine ("what you probably need next")
- Procedure versioning and confidence decay

### Phase 3: Latent Intelligence (Weeks 9-14) — "It understands"
- **JEPA encoder** (~5M params): replaces vector cosine with latent prediction retrieval
- **World Model** (~2-5M params): intent prediction ("you probably need X next")
- **Surprise-based ingestion**: replaces static confidence gate with novelty detection
- Offline GRPO optimization over trajectory batches
- Cross-session pattern detection via learned latent space

### Phase 4: Temporal Intelligence (Weeks 15-20) — "It compresses"
- **SSM temporal memory** (Mamba): constant-time temporal queries over full history
- **Memory-R1**: RL-based memory CRUD agent (ADD/UPDATE/DELETE/NOOP)
- **Graph-R1**: Multi-turn RL reasoning over knowledge graph
- SSM potentially replaces UCB bandit with learned temporal policy

### Phase 5: Enterprise (Weeks 21+) — "It scales"
- Docker packaging for team/enterprise deployment
- Multi-user isolation with shared knowledge graphs
- SOC2/GDPR compliance toolkit
- Merkle-chain audit log with cryptographic verification
- Policy-as-Code engine for enterprise governance rules

---

## 10. What This Document Supersedes

This consolidated vision replaces and unifies:
- `Project Cloak: A High-Performance, Privacy-First Agentic Memory Platform` (all versions)
- `Technical Architecture Document` (all versions)
- `Technical Architecture for Agentic AI Governance`
- `TraceMind PRD v1 and v2`
- `AgentMem v2 README`

All future product, engineering, and business documents derive from this single source of truth.
