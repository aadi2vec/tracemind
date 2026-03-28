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

## 3. The Unified Architecture

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
|  Phase 1: Vector Search | Phase 2: Graph Traversal               |
|  Phase 3: Cluster Expansion | Bounded recall budget              |
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
|  Phase 3: RL-based memory CRUD (Memory-R1 style)                |
+------------------------------------------------------------------+
```

---

## 4. Core Design Principles

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
- Rust core: memory engine (graph + vector + trace store)
- Canonicalization pipeline (entity extraction, triple generation)
- Governance funnel (PII filter, schema validation, user rules)
- UCB bandit retrieval controller
- Tauri desktop app with memory graph visualization
- Chrome extension for passive capture
- Claude Code MCP server for memory queries

### Phase 2: Learning (Weeks 5-8) — "It gets smarter"
- Trajectory storage for all interactions
- Feedback loop: user ratings → bandit policy updates
- Lightweight policy network trained on stored trajectories (idle-time)
- Context continuation engine ("what you probably need next")
- Claude Code hook for passive session recording

### Phase 3: RL Reasoning (Weeks 9-14) — "It reasons"
- Memory-R1: RL-based memory CRUD decisions
- Graph-R1: Multi-turn reasoning over the knowledge graph
- Offline GRPO optimization over trace batches
- Cross-session pattern detection

### Phase 4: Enterprise (Weeks 15+) — "It scales"
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
