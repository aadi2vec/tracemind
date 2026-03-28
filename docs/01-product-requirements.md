# TraceMind: Product Requirements Document

**Version:** 1.0
**Author:** Aaditya Srivathsan
**Date:** 2026-03-28
**Status:** Draft
**Derives from:** `docs/00-consolidated-vision.md`

---

## 1. Executive Summary

TraceMind is a local-only, privacy-first memory operating system that watches what you do on your computer (with explicit opt-in permission), extracts structured understanding from your digital activity, learns what works over time, and makes every AI tool you use smarter -- without any data ever leaving your machine.

The product ships as a lightweight Tauri desktop application (Rust backend, web frontend) for consumers, with a Docker container option for enterprise teams. It integrates with Claude Code bidirectionally: passive hooks record sessions into structured memory, and an active MCP server lets Claude Code query TraceMind for context, decisions, and learned preferences.

TraceMind is not a screen recorder. It is not a vector database with a UI. It is a **structured memory system with governance, audit trails, and reinforcement-learning-based self-improvement** -- all running on a consumer laptop within a 200MB RAM idle budget.

**Target launch:** Consumer beta within 8 weeks of development start. Enterprise preview at week 16.

---

## 2. Problem Statement

### The Core Problem

Every AI tool you use today has amnesia. Claude Code forgets your codebase conventions between sessions. Your browser history is a flat list of URLs with no structure. Your note-taking app stores text but cannot reason over it. No tool connects the dots across your digital activity.

### Why This Matters Now

| Pain Point | Impact | Who Feels It |
|---|---|---|
| AI assistants rediscover context every session | 10-15 min wasted per session re-explaining preferences, conventions, prior decisions | All AI users |
| No audit trail for AI-assisted decisions | Cannot explain why an AI suggested X; compliance risk in regulated industries | ML engineers, enterprise teams |
| Browser history / notes are unstructured | Cannot ask "why did I decide X?" and get a traced answer | Consumers, knowledge workers |
| Existing memory tools are cloud-first | Privacy-sensitive users and enterprises cannot adopt them | Everyone with data sensitivity |
| Screen recorders capture everything, understand nothing | Petabytes of video with no structure, no reasoning, no learning | Rewind/Limitless users |

### The Opportunity

A local-only system that builds structured understanding (entity graphs, semantic clusters, causal chains) from your digital activity, learns from outcomes, and surfaces the right context at the right time -- with full transparency into what it knows and why.

---

## 3. Target Users & Personas

### Primary: The Intentful Consumer ("Maya")

| Attribute | Detail |
|---|---|
| **Who** | Knowledge worker, 25-45, uses browser + AI tools daily |
| **Goal** | Wants her digital activity to compound into a personal knowledge system without manual effort |
| **Behavior** | Researches topics across tabs, uses Claude/ChatGPT, takes notes sporadically, forgets where she read things |
| **Frustration** | "I know I researched this last month but I can't find it. My browser history is useless." |
| **Privacy stance** | Will NOT use cloud tools for personal knowledge. Wants to see exactly what is stored. |
| **Wow moment** | "TraceMind surfaced the article I read 2 weeks ago that's directly relevant to what I'm working on right now." |
| **Technical skill** | Can install a desktop app and browser extension. Cannot configure Docker or write config files. |

### Secondary: The Claude Code Power User ("Raj")

| Attribute | Detail |
|---|---|
| **Who** | Software engineer, uses Claude Code 4+ hours/day |
| **Goal** | Wants Claude Code to accumulate knowledge of his codebase, preferences, and past decisions |
| **Behavior** | Works across multiple repos, has strong conventions, corrects Claude Code frequently |
| **Frustration** | "Claude Code asks me the same questions every session. It doesn't learn." |
| **Privacy stance** | Code is proprietary. Absolutely no cloud. |
| **Wow moment** | "Claude Code remembered that we decided to use the repository pattern 3 weeks ago and applied it without asking." |
| **Technical skill** | Comfortable with CLI, MCP configuration, Docker. |

### Tertiary: The ML Engineer ("Priya")

| Attribute | Detail |
|---|---|
| **Who** | ML/AI engineer building agent systems |
| **Goal** | Wants a structured memory backend with trajectory storage for training RL-based agents |
| **Behavior** | Builds custom agent loops, needs programmatic memory access, stores and replays decision trajectories |
| **Frustration** | "mem0 and Zep are just vector stores. I need graph structure, provenance, and trajectory data for RL training." |
| **Privacy stance** | Needs local-only for client work. Enterprise deployment for team use. |
| **Wow moment** | "I can train a policy network on TraceMind's stored trajectories and my agent's retrieval accuracy improved 30%." |
| **Technical skill** | Expert. Will use REST API and Python SDK directly. |

---

## 4. Product Vision

### Vision Statement

TraceMind makes your digital life cumulative. Every article you read, every decision you make, every AI conversation you have builds a structured, private, auditable knowledge system that makes you and your AI tools measurably smarter over time.

### Design Principles (Non-Negotiable)

| # | Principle | Implication |
|---|---|---|
| P1 | **Local-only execution** | No cloud calls. No telemetry. No server to send data to. This is architecture, not policy. |
| P2 | **Auditability above convenience** | Every decision is reproducible from its trace. Every memory write has provenance. |
| P3 | **Lightweight by design** | <200MB idle RAM, <500MB active, <250MB install. Embedded databases only. No JVM, no server processes. |
| P4 | **Structure controls learning** | Memory is typed, timestamped, confidence-scored triples in a graph -- never unstructured blobs. |
| P5 | **Human corrections are first-class** | User corrections go through the full canonicalization and governance pipeline and become permanent graph updates. |
| P6 | **Graceful degradation** | Every advanced feature has a rule-based fallback. Works on day one with zero training data. |
| P7 | **Transparency is the product** | Users can always see what TraceMind knows, how it learned it, and why it surfaced something. |

---

## 5. Core Features (MVP)

The MVP is scoped to deliver the first two "wow moments" -- "It remembered" and "Show me why" -- within Phase 1 (weeks 1-4).

| # | Feature | Priority | Persona Served | Wow Moment |
|---|---|---|---|---|
| F1 | Passive capture pipeline (browser extension + clipboard + file watcher) | P0 | Maya | "It remembers what I browsed" |
| F2 | Canonicalization engine (entity extraction, triple generation, embedding) | P0 | All | Foundation for all features |
| F3 | Governance funnel (PII filter, schema validation, user-defined rules) | P0 | All | Trust mechanism |
| F4 | Structured memory layer (entity graph + vector store + episodic traces) | P0 | All | Foundation for all features |
| F5 | Multi-phase retrieval engine (vector + graph + cluster expansion) | P0 | All | "It finds the right thing" |
| F6 | Claude Code MCP server (query memory, write memory) | P0 | Raj | "Claude Code remembers" |
| F7 | Claude Code hooks (passive session recording) | P0 | Raj | "Every session is captured" |
| F8 | Tauri desktop app with memory graph visualization | P0 | Maya | "I can see what it knows" |
| F9 | Trace replay (show full provenance for any retrieved memory) | P1 | All | "Show me why" |
| F10 | UCB bandit retrieval controller (learns which retrieval strategies work) | P1 | Raj, Priya | "It gets smarter" |
| F11 | Feedback loop (user ratings flow into bandit policy updates) | P1 | All | "It learns from me" |
| F12 | Chrome extension for passive URL/selection/search capture | P0 | Maya | "It watches what I browse" |

---

## 6. Feature Details with Acceptance Criteria

### F1: Passive Capture Pipeline

**Description:** Continuously monitors opted-in data sources and feeds raw observations into the canonicalization engine. All capture is opt-in per source, with a visible system tray indicator when active.

**Data Sources (MVP):**

| Source | What is Captured | Default State |
|---|---|---|
| Chrome extension | URL, page title, selected text, search queries | Opt-in per domain |
| Clipboard | Text copied to clipboard (text only, no images in v1) | Off by default |
| File watcher | File creation/modification events in specified directories | Off by default |
| Claude Code hooks | Full session transcripts, file changes, tool invocations | On when TraceMind is running |

**Acceptance Criteria:**

- [ ] AC1.1: System tray icon shows green/amber/red status indicating capture state (active/partial/off).
- [ ] AC1.2: User can toggle each capture source independently from the system tray menu.
- [ ] AC1.3: Chrome extension captures page URL and title on every navigation. Selected text is captured only when user explicitly highlights and right-clicks "Save to TraceMind."
- [ ] AC1.4: Clipboard monitoring captures text only. Binary data, images, and passwords (detected via heuristic: high entropy, short length, no spaces) are silently dropped.
- [ ] AC1.5: File watcher monitors only user-specified directories. Default: none. Changes are debounced (5-second window) to avoid duplicate events.
- [ ] AC1.6: All captured data includes a `source_type`, `source_id`, and `captured_at` timestamp.
- [ ] AC1.7: Capture pipeline processes events within 2 seconds of occurrence (p95 latency).
- [ ] AC1.8: Capture can be globally paused/resumed with a single keyboard shortcut (default: Cmd+Shift+T / Ctrl+Shift+T).

---

### F2: Canonicalization Engine

**Description:** Transforms raw captured text into structured facts: named entities, subject-predicate-object triples, and vector embeddings. Runs entirely on-device using a quantized embedding model and local NLP.

**Acceptance Criteria:**

- [ ] AC2.1: Entity extraction identifies Person, Organization, Technology, Concept, File, URL, and Event entity types from raw text.
- [ ] AC2.2: Triple generation produces (subject, predicate, object) triples with confidence scores in [0.0, 1.0].
- [ ] AC2.3: Embedding uses all-MiniLM-L6-v2 via ONNX Runtime. Model size < 100MB. Inference latency < 50ms per text chunk on CPU.
- [ ] AC2.4: Deduplication detects and merges entities that refer to the same real-world thing (e.g., "JS" and "JavaScript") using embedding cosine similarity > 0.92 as merge threshold.
- [ ] AC2.5: Every generated triple includes a `source_id` linking back to the raw capture event.
- [ ] AC2.6: Canonicalization is idempotent: processing the same raw input twice produces no duplicate entities or triples.
- [ ] AC2.7: The engine processes a 500-word text chunk in < 200ms end-to-end on a 2021 MacBook Air (M1, 8GB RAM).

---

### F3: Governance Funnel

**Description:** All data passes through a mandatory governance funnel before reaching memory storage. This funnel applies PII detection, schema validation, user-defined rules, and consent verification. Data that fails governance is quarantined, not silently dropped.

**Acceptance Criteria:**

- [ ] AC3.1: PII detection identifies and redacts email addresses, phone numbers, SSNs, credit card numbers, and API keys/tokens using regex patterns and entropy analysis. False positive rate < 5%.
- [ ] AC3.2: User can define inclusion/exclusion rules via a simple rule DSL in the settings UI. Example: `exclude domain:*.bank.com`, `exclude entity_type:CreditCard`, `include directory:/Projects`.
- [ ] AC3.3: Every piece of data that enters memory has a `governance_status` field: `approved`, `redacted`, or `quarantined`.
- [ ] AC3.4: Quarantined items appear in a "Review Queue" in the desktop app. User can approve, permanently delete, or modify before storage.
- [ ] AC3.5: Governance rules are version-controlled. Changing a rule does NOT retroactively modify already-stored data. User can trigger a manual "re-governance" scan if desired.
- [ ] AC3.6: An append-only governance audit log records every filter decision with timestamp, rule applied, and outcome. This log is viewable in the desktop app.
- [ ] AC3.7: Governance processing adds < 10ms latency per item (p95).

---

### F4: Structured Memory Layer

**Description:** Three complementary storage systems that together form TraceMind's persistent memory.

**Sub-components:**

| Store | Technology | Purpose | Data Model |
|---|---|---|---|
| Entity Graph | Kuzu (embedded) | Typed relationships between entities | Nodes (Entity) + Edges (Triplet with timestamp, confidence, source_id) |
| Vector Store | LanceDB (embedded) | Semantic similarity search | Embedding vectors + metadata (source_id, timestamp, text chunk) |
| Episodic Trace Store | Append-only Parquet files | Full provenance for every decision | ContextTrace records (see schema.py) |

**Acceptance Criteria:**

- [ ] AC4.1: Entity graph supports CREATE, READ, UPDATE, DELETE for both nodes and edges. All mutations are atomic.
- [ ] AC4.2: Vector store indexes embeddings with < 100ms query latency for top-10 nearest neighbors on a corpus of 100K vectors.
- [ ] AC4.3: Episodic trace store is append-only. No trace record can be modified or deleted (only new records can annotate old ones).
- [ ] AC4.4: All three stores share a consistent `source_id` scheme for cross-referencing. Given any trace, a user can navigate to the entities, triples, and vectors it references.
- [ ] AC4.5: Total disk usage for 1 year of moderate consumer use (50 captures/day) is < 2GB.
- [ ] AC4.6: Confidence decay: entity and triple confidence scores decay by a configurable rate (default: 0.995 per day). Items below a configurable threshold (default: 0.1) are flagged for garbage collection but NOT automatically deleted.
- [ ] AC4.7: All stores are backed up atomically via a single "Export Memory" action in the desktop app, producing a portable archive.

---

### F5: Multi-Phase Retrieval Engine

**Description:** Given a query, retrieval proceeds in three bounded phases: (1) vector similarity search, (2) graph traversal from vector hits, (3) cluster expansion. Each phase has a configurable budget (max items returned). The retrieval controller selects which phases to run and in what order based on a learned policy (UCB bandit in v1).

**Acceptance Criteria:**

- [ ] AC5.1: Phase 1 (Vector): Returns top-K (default K=20) semantically similar chunks. Latency < 100ms.
- [ ] AC5.2: Phase 2 (Graph): Starting from entities mentioned in Phase 1 results, traverses up to 2 hops in the entity graph. Returns connected entities and triples. Budget: max 50 additional facts.
- [ ] AC5.3: Phase 3 (Cluster): Expands to entities in the same semantic cluster as Phase 1+2 results. Budget: max 20 additional items.
- [ ] AC5.4: Total retrieval budget is capped at 100 items per query (configurable). The retrieval controller allocates budget across phases.
- [ ] AC5.5: Every retrieved item includes its `source_id`, retrieval phase, and relevance score. This provenance is stored in the `ContextTrace`.
- [ ] AC5.6: End-to-end retrieval latency (all three phases) < 500ms (p95) on a corpus of 100K items.
- [ ] AC5.7: If the retrieval controller has insufficient training data (< 50 queries), it falls back to a fixed allocation: 60% vector, 30% graph, 10% cluster.

---

### F6: Claude Code MCP Server

**Description:** An MCP (Model Context Protocol) server that Claude Code connects to, enabling bidirectional memory access. Claude Code can query TraceMind for relevant context and write new memories from session outcomes.

**MCP Tools Exposed:**

| Tool Name | Description | Parameters |
|---|---|---|
| `tracemind_query` | Retrieve relevant memories for a query | `query: string`, `max_results: int`, `include_provenance: bool` |
| `tracemind_remember` | Store a new fact or decision | `content: string`, `entities: string[]`, `confidence: float` |
| `tracemind_recall_session` | Get summary of a past session by date or topic | `topic: string`, `date_range: {start, end}` |
| `tracemind_get_preferences` | Get user preferences relevant to a context | `context: string` |
| `tracemind_feedback` | Record whether a retrieved memory was helpful | `trace_id: string`, `score: float`, `correction: string?` |

**Acceptance Criteria:**

- [ ] AC6.1: MCP server starts automatically when TraceMind desktop app is running. Listens on a Unix domain socket (macOS/Linux) or named pipe (Windows).
- [ ] AC6.2: `tracemind_query` returns results in < 600ms (p95), including provenance when requested.
- [ ] AC6.3: `tracemind_remember` processes a new memory and returns confirmation within 1 second. The memory is immediately queryable.
- [ ] AC6.4: `tracemind_feedback` updates the UCB bandit's reward signal for the retrieval arm used in the corresponding trace.
- [ ] AC6.5: All MCP interactions are logged in the episodic trace store with `source_type: "claude_code_mcp"`.
- [ ] AC6.6: MCP server handles concurrent requests from multiple Claude Code sessions without data corruption. Minimum: 10 concurrent connections.
- [ ] AC6.7: If TraceMind is not running, Claude Code gracefully degrades (MCP connection timeout < 2 seconds, no crash, no hang).

---

### F7: Claude Code Hooks (Passive Session Recording)

**Description:** Shell hooks that fire on Claude Code session start/end, capturing the full session transcript, file changes, tool invocations, and user corrections into TraceMind's memory.

**Acceptance Criteria:**

- [ ] AC7.1: Hook installs via a single CLI command: `tracemind hooks install`. Modifies Claude Code's `settings.json` to register the hooks.
- [ ] AC7.2: On session start, a new episodic trace is created with `source_type: "claude_code_session"`, `task_id`, and start timestamp.
- [ ] AC7.3: On session end, the trace is finalized with: files changed (paths, not content diffs for v1), tools invoked, user corrections made, and session duration.
- [ ] AC7.4: Session data flows through the full canonicalization + governance pipeline. PII in code snippets is handled per governance rules.
- [ ] AC7.5: Hook overhead adds < 100ms to session start and < 500ms to session end.
- [ ] AC7.6: Hooks can be disabled without uninstalling TraceMind: `tracemind hooks disable`.

---

### F8: Tauri Desktop App

**Description:** The primary user interface. A cross-platform desktop application built with Tauri (Rust backend + web frontend) that provides memory visualization, governance management, and system status.

**Key Views:**

| View | Purpose |
|---|---|
| **Dashboard** | System status, capture indicators, recent activity feed, learning progress |
| **Memory Graph** | Interactive force-directed graph of entities and relationships. Click any node to see provenance. |
| **Timeline** | Chronological view of captured events and decisions. Filterable by source, entity, date. |
| **Trace Replay** | Select any decision/retrieval and see the full trace: what was queried, what was retrieved, why, and the outcome. |
| **Governance** | Active rules, PII filter stats, quarantine review queue, audit log. |
| **Settings** | Capture source toggles, governance rule editor, performance tuning, data export/import. |

**Acceptance Criteria:**

- [ ] AC8.1: App launches in < 3 seconds on a 2021 MacBook Air (M1, 8GB).
- [ ] AC8.2: Memory graph renders up to 500 nodes and 2000 edges interactively at 30fps. Larger graphs use progressive disclosure (expand on click).
- [ ] AC8.3: Timeline view loads the last 7 days of activity in < 1 second. Infinite scroll for older data.
- [ ] AC8.4: Trace replay shows: input query, retrieval phases executed, items retrieved (with scores), reasoning steps, final output, and outcome/feedback (if available).
- [ ] AC8.5: All views support search with < 200ms response time.
- [ ] AC8.6: App is fully functional offline (it is always offline -- no network calls ever).
- [ ] AC8.7: Install size (app + embedding model + empty databases) < 250MB.
- [ ] AC8.8: Idle RAM usage (app open, no active query) < 200MB. Active RAM (during query/rendering) < 500MB.

---

### F9: Trace Replay

**Description:** Given any memory retrieval or decision that TraceMind contributed to, the user can "replay" the full provenance chain: what was the query, what retrieval strategy was selected, what items were retrieved and scored, what reasoning was applied, and what the outcome was.

**Acceptance Criteria:**

- [ ] AC9.1: Every retrieval event generates a `ContextTrace` record (per existing schema) that is permanently stored.
- [ ] AC9.2: Trace replay UI shows a step-by-step visualization: Query --> Retrieval Arm Selected --> Phase 1 Results --> Phase 2 Results --> Phase 3 Results --> Final Context --> Output.
- [ ] AC9.3: Each step in the replay is clickable to drill into detail (e.g., click a retrieved item to see its full entity, source, and capture context).
- [ ] AC9.4: Traces are searchable by date, query text, entities involved, and outcome (success/failure/unknown).
- [ ] AC9.5: Exporting a trace produces a self-contained JSON file that can be shared (with governance-compliant redactions applied automatically).

---

### F10: UCB Bandit Retrieval Controller

**Description:** The retrieval controller uses an Upper Confidence Bound (UCB) multi-armed bandit to learn which retrieval strategy (vector-heavy, graph-heavy, cluster-heavy, balanced) works best for different query types. It updates from user feedback and outcome signals.

**Acceptance Criteria:**

- [ ] AC10.1: The bandit maintains at least 4 arms representing different retrieval budget allocations across vector, graph, and cluster phases.
- [ ] AC10.2: Arm selection uses UCB1 formula with exploration bonus. Exploration parameter is configurable (default: sqrt(2)).
- [ ] AC10.3: After each retrieval, the outcome (user feedback score or downstream task success) updates the selected arm's reward estimate.
- [ ] AC10.4: With < 50 total observations, the controller uses a fixed fallback allocation (60/30/10) instead of bandit selection.
- [ ] AC10.5: Bandit state persists across app restarts. State is stored in a < 1KB file.
- [ ] AC10.6: A "Learning Progress" chart in the dashboard shows cumulative reward and arm selection distribution over time.

---

### F11: Feedback Loop

**Description:** Users provide explicit feedback on TraceMind's retrievals via thumbs up/down in the desktop app or via the `tracemind_feedback` MCP tool. This feedback flows into the bandit controller and trajectory storage.

**Acceptance Criteria:**

- [ ] AC11.1: Every retrieval result in the desktop app has a thumbs up/down button. Optional text correction field.
- [ ] AC11.2: Feedback is recorded as a `Feedback` record (per existing schema) linked to the `ContextTrace`.
- [ ] AC11.3: Positive feedback (score > 0) increments the bandit arm's reward. Negative feedback (score < 0) decrements it.
- [ ] AC11.4: User corrections (text field) are processed through the canonicalization pipeline and create new graph updates -- not just metadata annotations.
- [ ] AC11.5: The system records a (state, action, reward, next_state) trajectory tuple for every feedback event, stored for future RL training.

---

### F12: Chrome Extension

**Description:** A Manifest V3 Chrome extension that captures browsing activity and sends it to the local TraceMind instance. No data leaves the browser except to localhost.

**Acceptance Criteria:**

- [ ] AC12.1: Extension communicates with TraceMind via localhost HTTP (127.0.0.1 only) or native messaging. No external network requests.
- [ ] AC12.2: Captures: page URL, page title, timestamp on every navigation event.
- [ ] AC12.3: Captures selected text only when user right-clicks and selects "Save to TraceMind" from context menu.
- [ ] AC12.4: User can configure domain allowlist/blocklist in the extension popup. Default: all domains captured for URL/title, none for full text.
- [ ] AC12.5: Extension shows a badge indicating connection status to TraceMind (green = connected, red = disconnected).
- [ ] AC12.6: Extension adds < 5MB to Chrome's memory footprint.
- [ ] AC12.7: All captured data includes `source_type: "chrome_extension"` and `source_id` linking to the specific page visit.

---

## 7. Non-Functional Requirements

### 7.1 Performance

| Metric | Target | Measurement |
|---|---|---|
| Install size (app + model + empty DBs) | < 250MB | Installer package size |
| RAM at idle (app open, no query) | < 200MB | macOS Activity Monitor resident size |
| RAM under active use (query + graph render) | < 500MB | Peak resident size during benchmark |
| App cold start | < 3 seconds | Time from launch to interactive dashboard |
| Capture-to-memory latency | < 2 seconds (p95) | Time from browser event to graph node creation |
| Retrieval latency (end-to-end) | < 500ms (p95) | Time from query to ranked results returned |
| Embedding inference | < 50ms per chunk | ONNX Runtime on CPU, all-MiniLM-L6-v2 |
| Disk growth rate | < 5MB/day typical use | 50 captures/day, moderate browsing |
| 1 year storage | < 2GB | Cumulative across all stores |

### 7.2 Security

| Requirement | Implementation |
|---|---|
| No network egress | Rust binary makes zero network calls. Verified by firewall rules in CI. |
| No telemetry | No analytics, no crash reporting, no update checks (updates are manual). |
| Data at rest encryption | SQLite/Kuzu databases encrypted with user-provided passphrase via SQLCipher or equivalent. Optional, off by default for performance. |
| Memory wipe | "Factory Reset" button that cryptographically shreds all stored data (overwrite + delete). |
| Process isolation | Tauri's WebView runs with restricted permissions. No file system access from frontend JS except via Rust IPC. |
| Extension security | Chrome extension uses Manifest V3 with minimal permissions: `activeTab`, `contextMenus`, `storage`. No `<all_urls>` in production. |

### 7.3 Privacy

| Requirement | Implementation |
|---|---|
| Opt-in capture only | No data source is active by default except Claude Code hooks (when TraceMind is running). |
| Visible capture indicator | System tray icon changes color when any capture is active. |
| Granular consent | Per-source, per-domain, per-directory consent toggles. |
| PII filtering | Mandatory governance funnel with regex + entropy-based PII detection. |
| Right to delete | User can delete any entity, triple, or trace. Deletion is propagated across all stores. |
| Data portability | Full memory export as a portable archive (JSON + Parquet + embeddings). |
| No ambient listening | No microphone, no camera, no screen recording. Text-only capture in v1. |

### 7.4 Compatibility

| Platform | Minimum Version | Architecture |
|---|---|---|
| macOS | 12.0 (Monterey) | x86_64, aarch64 (Apple Silicon native) |
| Windows | 10 (21H2) | x86_64 |
| Linux | Ubuntu 22.04 / Fedora 38 equivalent | x86_64, aarch64 |

### 7.5 Reliability

| Requirement | Target |
|---|---|
| Crash-free sessions | > 99.5% of sessions complete without crash |
| Data integrity on crash | All stores use WAL or append-only writes. No data loss on unclean shutdown. |
| Graceful degradation | If embedding model fails to load, system operates in keyword-only mode. If graph DB fails, vector-only retrieval. |

---

## 8. User Journeys

### Journey 1: Maya Researches a Topic Over Multiple Days

**Trigger:** Maya is evaluating whether to migrate her team's database from MySQL to PostgreSQL.

**Day 1 (Tuesday):**
1. Maya opens Chrome. TraceMind extension is active (green badge).
2. She searches "MySQL vs PostgreSQL 2026 comparison" -- search query captured.
3. She reads 4 articles. URLs and titles captured automatically.
4. She highlights a key benchmark result and right-clicks "Save to TraceMind." The selected text is captured.
5. TraceMind's canonicalization engine extracts entities (MySQL, PostgreSQL, benchmark metrics) and generates triples: `(PostgreSQL, outperforms, MySQL)` with confidence 0.85, `(PostgreSQL, supports, JSONB)` with confidence 0.95.
6. Maya sees a brief notification: "3 new entities, 7 new facts stored."

**Day 3 (Thursday):**
7. Maya opens TraceMind desktop app to review what she's gathered.
8. She navigates to the Memory Graph view and sees a cluster around "database migration" with connected entities.
9. She clicks on the PostgreSQL node and sees all captured facts with provenance (which article, what date, what she highlighted).
10. She notices a triple she disagrees with and clicks to correct it. The correction enters the governance pipeline and updates the graph.

**Day 5 (Saturday):**
11. Maya opens Claude Code to write a migration plan.
12. Claude Code (via MCP) queries TraceMind: "What do we know about database migration?"
13. TraceMind returns the structured research: entities, facts, sources, and Maya's correction.
14. Claude Code drafts a migration plan that references her specific research findings.
15. Maya is surprised: "It remembered everything I read this week."

**Verification Points:**
- At step 5, entities appear in the graph within 2 seconds of capture.
- At step 9, provenance links trace back to specific articles and timestamps.
- At step 10, correction flows through governance and updates the graph (not just a metadata tag).
- At step 13, retrieval includes provenance and confidence scores.

---

### Journey 2: Raj Uses Claude Code Across Multiple Sessions

**Trigger:** Raj is building an authentication module for a new project. He uses Claude Code daily.

**Session 1 (Monday):**
1. Raj starts a Claude Code session. TraceMind hook fires, creating a new episodic trace.
2. Raj and Claude Code discuss auth approaches. They decide on JWT with refresh tokens.
3. Raj corrects Claude Code: "We always use bcrypt, not argon2, in this org."
4. Session ends. Hook captures: decision (JWT + bcrypt), files created, correction made.
5. TraceMind extracts entities (JWT, bcrypt, auth-module) and triples: `(auth-module, uses, JWT)`, `(org, prefers, bcrypt-over-argon2)`.

**Session 5 (Wednesday):**
6. Raj starts a new session on the same project.
7. Claude Code (via MCP) automatically queries TraceMind for context on this project.
8. TraceMind returns: "Auth module uses JWT with refresh tokens. Org prefers bcrypt. Decided on Monday."
9. Claude Code applies these conventions without asking Raj again.
10. Raj asks Claude Code to add password reset. Claude Code generates code using bcrypt (not argon2) without being told.
11. Raj is delighted: "It actually learned."

**Session 20 (Next week):**
12. Raj opens TraceMind dashboard and navigates to the "Learning Progress" chart.
13. He sees that retrieval accuracy (based on his feedback) has improved from 60% to 85% over 20 sessions.
14. He clicks on a specific retrieval that was marked "unhelpful" and sees the full trace: what was queried, what was retrieved, and why it was wrong.
15. The bandit has shifted allocation toward graph-heavy retrieval for this project (because entity relationships matter more than raw text similarity for code).

**Verification Points:**
- At step 5, session data enters memory within 500ms of session end.
- At step 8, retrieval includes the correction from Session 1 with provenance.
- At step 10, preference is applied correctly (bcrypt, not argon2).
- At step 13, learning progress is visible and quantified.

---

### Journey 3: Priya Builds a Custom Agent with TraceMind Backend

**Trigger:** Priya is building an agent that helps her team triage customer support tickets. She wants the agent to learn which triaging decisions worked.

**Setup:**
1. Priya installs TraceMind via Docker (enterprise mode).
2. She connects her agent's Python code to TraceMind's REST API.
3. She configures a governance rule: `exclude entity_type:CustomerPII`, `exclude entity_type:CreditCard`.

**Agent Operation:**
4. Her agent processes ticket #1042. It queries TraceMind: "Similar past tickets about billing errors."
5. TraceMind returns 5 relevant past tickets with their triage decisions and outcomes.
6. The agent triages the ticket as "Priority: High, Route: Billing Team."
7. Priya's agent writes back: `tracemind_remember("Ticket #1042 triaged as High/Billing. Customer had subscription renewal issue.")`.
8. Two days later, the ticket is resolved successfully. The agent records: `tracemind_feedback(trace_id="...", score=1.0)`.

**Learning Loop:**
9. After 200 tickets, Priya exports TraceMind's trajectory data (state, action, reward tuples).
10. She trains a lightweight policy network offline on these trajectories.
11. She deploys the trained policy back into TraceMind's retrieval controller.
12. Triage accuracy improves from 72% to 89% on a held-out test set.

**Verification Points:**
- At step 3, PII governance rules prevent customer personal data from entering the graph.
- At step 5, retrieval includes outcome data from past triages.
- At step 9, trajectory export produces a clean (state, action, reward, next_state) dataset.
- At step 11, the custom policy can replace the UCB bandit without system restart.

---

## 9. Success Metrics

### North Star Metric

**Retrieval Helpfulness Rate:** Percentage of TraceMind retrievals that users rate as helpful (thumbs up) or that lead to successful downstream outcomes. Target: >80% after 30 days of use.

### Primary Metrics

| Metric | Target (30 days) | Target (90 days) | Measurement |
|---|---|---|---|
| Daily Active Users (desktop app opened) | 60% of installs | 50% of installs | App launch telemetry (local-only counter, never transmitted) |
| Memories stored per user per day | > 30 | > 50 | Count of new graph nodes + triples |
| Retrieval helpfulness rate | > 65% | > 80% | Feedback score aggregation |
| Claude Code MCP queries per session | > 3 | > 5 | MCP server request log |
| Trace replay usage | > 1 per week per user | > 3 per week per user | UI event log (local) |

### Guardrail Metrics (Must Not Exceed)

| Metric | Threshold | Action if Exceeded |
|---|---|---|
| App RAM (idle) | 250MB | Performance regression investigation |
| App RAM (active) | 600MB | Memory leak investigation |
| Retrieval latency (p95) | 1 second | Retrieval engine optimization sprint |
| Disk growth rate | 10MB/day | Compression or retention policy review |
| Capture pipeline error rate | 2% | Pipeline reliability fix |
| Governance false positive rate (PII) | 10% | PII model tuning |

---

## 10. Risks & Mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | **Local embedding models produce poor entity extraction on consumer hardware** | Medium | High | Ship with multiple model sizes (nano/small/base). Benchmark on M1 MacBook Air. Allow user to select quality vs. speed tradeoff. |
| R2 | **Privacy backlash despite opt-in design** | Medium | Critical | Transparent governance UI. Mandatory onboarding that explains exactly what is captured. Prominent "pause all capture" button. Open-source the governance funnel. |
| R3 | **RAM budget exceeded on lower-end machines** | High | High | Aggressive lazy loading. Memory-mapped databases. Configurable "lite mode" that disables graph rendering and reduces cache sizes. Profile on 8GB M1 continuously in CI. |
| R4 | **UCB bandit does not converge for users with few interactions** | Medium | Medium | Rule-based fallback for < 50 interactions. Progressive disclosure: bandit only activates after sufficient data, with clear UI indication. |
| R5 | **Chrome extension rejected from Web Store** | Low | Medium | Ensure Manifest V3 compliance. Minimal permissions. Provide sideload instructions as fallback. Target Firefox as secondary browser. |
| R6 | **Kuzu or LanceDB has breaking changes or is abandoned** | Low | High | Abstraction layer over graph and vector stores. Can swap to SQLite FTS5 + faiss-rs as fallback. No direct DB API calls from business logic. |
| R7 | **Tauri cross-platform inconsistencies** | Medium | Medium | CI builds and tests on macOS, Windows, Linux for every release. Platform-specific code paths isolated behind traits. |
| R8 | **User stores sensitive data despite governance filters** | Medium | High | PII detection is defense-in-depth, not foolproof. Encrypted-at-rest option. "Sensitive Memory" flag that requires re-authentication to view. Prominent data deletion tools. |
| R9 | **Adoption friction: too many setup steps** | Medium | High | One-click installer. Chrome extension auto-detects TraceMind on localhost. Claude Code hooks install via single CLI command. First-run wizard with < 3 steps. |
| R10 | **RL training on trajectories requires GPU** | Low | Medium | Phase 1-2 use CPU-only UCB bandit (no training needed). RL training (Phase 3) uses small networks trainable on CPU in < 10 minutes on stored trajectories. Optional GPU acceleration for power users. |

---

## 11. Phased Rollout Plan

### Phase 1: Foundation (Weeks 1-4) -- "It Remembers"

**Goal:** Capture, store, retrieve, and visualize. The core memory loop works end-to-end.

| Deliverable | Owner | Week |
|---|---|---|
| Rust core: graph store (Kuzu), vector store (LanceDB), trace store (Parquet) | Backend | 1-2 |
| Canonicalization pipeline (entity extraction, triple generation, ONNX embedding) | Backend | 1-2 |
| Governance funnel (PII filter, schema validation, rule engine) | Backend | 2-3 |
| Claude Code MCP server (query + write) | Backend | 2-3 |
| Claude Code hooks (session recording) | Backend | 3 |
| Chrome extension (URL/title capture, context menu "Save to TraceMind") | Frontend | 2-3 |
| Tauri desktop app: Dashboard, Memory Graph, Settings | Frontend | 2-4 |
| Clipboard monitor + file watcher | Backend | 3-4 |
| End-to-end integration testing | QA | 4 |

**Exit Criteria:**
- A user can browse the web, have their activity captured, and see structured entities/relationships in the desktop app.
- Claude Code can query and write to TraceMind via MCP.
- Governance funnel blocks PII with < 5% false positive rate.
- All performance targets met on reference hardware (M1 MacBook Air, 8GB).

### Phase 2: Learning (Weeks 5-8) -- "It Gets Smarter"

**Goal:** The system improves with use. Users see measurable learning progress.

| Deliverable | Owner | Week |
|---|---|---|
| UCB bandit retrieval controller (replaces fixed allocation) | Backend | 5-6 |
| Feedback loop (thumbs up/down in UI, MCP feedback tool) | Full stack | 5-6 |
| Trajectory storage (state, action, reward tuples) | Backend | 6 |
| Trace replay UI | Frontend | 6-7 |
| Context continuation engine ("what you probably need next") | Backend | 7-8 |
| Learning progress dashboard | Frontend | 7-8 |
| Timeline view with filtering | Frontend | 7-8 |
| Performance optimization pass (memory profiling, lazy loading) | Backend | 8 |

**Exit Criteria:**
- Retrieval helpfulness rate improves measurably over 2 weeks of simulated use.
- User can see learning progress in the dashboard.
- Trace replay works for any past retrieval event.
- RAM stays within budget after 30 days of accumulated data.

### Phase 3: RL Reasoning (Weeks 9-14) -- "It Reasons"

**Goal:** Move from bandit-based learning to full RL-based memory management.

| Deliverable | Owner | Week |
|---|---|---|
| Memory-R1: RL-based memory CRUD decisions (store/update/forget/defer) | ML | 9-11 |
| Trajectory export for offline training | ML | 10 |
| Lightweight policy network (CPU-trainable on stored trajectories) | ML | 11-12 |
| Graph-R1: Multi-turn reasoning over the knowledge graph | ML | 12-14 |
| Cross-session pattern detection (proactive surfacing) | Backend | 13-14 |
| Plugin architecture for custom retrieval policies | Backend | 14 |

**Exit Criteria:**
- RL-trained policy outperforms UCB bandit by > 10% on retrieval helpfulness.
- Policy training completes in < 10 minutes on CPU with 30 days of trajectory data.
- Cross-session patterns are surfaced proactively with > 70% relevance rate.

### Phase 4: Enterprise (Weeks 15-20) -- "It Scales"

**Goal:** Multi-user deployment for teams with compliance requirements.

| Deliverable | Owner | Week |
|---|---|---|
| Docker container packaging (single-command deploy) | DevOps | 15 |
| Multi-user isolation (separate memory graphs, shared knowledge optional) | Backend | 15-17 |
| REST API with authentication (for programmatic access) | Backend | 16-17 |
| Policy-as-Code engine (enterprise governance rules) | Backend | 17-18 |
| Merkle-chain audit log with cryptographic verification | Backend | 18-19 |
| SOC2/GDPR compliance documentation and controls | Legal/Eng | 19-20 |
| Admin dashboard (user management, policy deployment, audit review) | Frontend | 19-20 |

**Exit Criteria:**
- 5-person team can use TraceMind concurrently with isolated memory and shared knowledge graph.
- Audit log passes cryptographic integrity verification.
- All compliance controls documented and testable.

---

## 12. Out of Scope (v1)

The following are explicitly NOT included in the MVP or Phase 1-2 releases:

| Item | Rationale | Planned Phase |
|---|---|---|
| **Screen recording / OCR** | Privacy risk; text-based capture is sufficient for MVP | Phase 5+ (if ever) |
| **Audio/voice capture** | Requires microphone permissions; too invasive for trust-building phase | Phase 5+ |
| **Image understanding** | Multimodal canonicalization is complex; text-first | Phase 3+ |
| **Mobile app** | Desktop-first; mobile adds platform complexity | Phase 5+ |
| **Cloud sync / multi-device** | Violates local-only principle. May explore encrypted sync later. | Never (or encrypted P2P in Phase 6+) |
| **Firefox / Safari extensions** | Chrome-first to reduce surface area | Phase 3 |
| **Automatic PII anonymization for sharing** | Complex; v1 supports manual redaction only | Phase 4 |
| **Natural language governance rules** | v1 uses simple DSL; NL parsing adds complexity and ambiguity | Phase 3 |
| **Custom embedding models** | Ship with one model; allow model swapping in Phase 3 | Phase 3 |
| **Collaborative memory (team knowledge graphs)** | Requires multi-user architecture | Phase 4 |
| **GPU acceleration** | CPU-only in v1. GPU optional for RL training in Phase 3. | Phase 3 |
| **Undo/redo for memory edits** | v1 memory edits are final (but traced). Undo requires CRDT-like complexity. | Phase 4 |
| **Plugin/extension marketplace** | Plugin architecture in Phase 3; marketplace is a distribution problem | Phase 5+ |

---

## 13. Open Questions

| # | Question | Impact | Owner | Due By |
|---|---|---|---|---|
| OQ1 | **Should the Chrome extension capture full page text by default, or only on explicit user action?** Full text gives richer memory but raises privacy expectations. | High -- affects trust model and storage growth | Product | Week 1 |
| OQ2 | **Kuzu vs. SQLite for the entity graph.** Kuzu is purpose-built for graphs but newer and less battle-tested. SQLite with a property-graph layer is more portable but slower for graph traversals. | High -- core infrastructure decision | Engineering | Week 1 |
| OQ3 | **How do we handle the first-run experience when memory is empty?** Cold-start problem: the system is least useful when it has the least data. Should we offer a "memory import" from browser history, notes apps, etc.? | Medium -- affects Day 1 experience | Product | Week 2 |
| OQ4 | **What is the right confidence decay rate?** Too fast and old knowledge disappears. Too slow and stale facts dominate. Should decay be uniform or entity-type-specific? | Medium -- affects memory quality over time | ML | Week 5 |
| OQ5 | **Should TraceMind expose a system prompt fragment for Claude Code**, or should Claude Code compose its own prompts from TraceMind's structured output? System prompt fragments are simpler but less flexible. | Medium -- affects MCP API design | Engineering | Week 2 |
| OQ6 | **How do we benchmark "retrieval helpfulness" before we have real users?** Need a synthetic evaluation dataset that simulates consumer browsing + Claude Code usage patterns. | High -- blocks learning loop validation | ML | Week 4 |
| OQ7 | **Should governance rules be exportable/importable?** Would allow enterprise admins to distribute policy sets. Adds complexity but high enterprise value. | Low (v1) / High (v4) | Product | Week 14 |
| OQ8 | **What happens when disk usage exceeds the 2GB soft limit?** Options: (a) warn user, (b) auto-archive old data, (c) increase compression, (d) garbage-collect low-confidence items. | Medium | Engineering | Week 6 |
| OQ9 | **Should we support Windows ARM (Snapdragon X)?** Growing market share but adds a build target. | Low | Engineering | Week 8 |
| OQ10 | **Licensing model: open-source core + commercial enterprise, or fully open-source?** Affects contribution model, enterprise sales, and trust narrative. | High -- business decision | Founders | Week 2 |

---

*This document is the authoritative product specification for TraceMind v1. All engineering, design, and business decisions should be traceable to requirements defined here. Updates require version increment and changelog entry.*
