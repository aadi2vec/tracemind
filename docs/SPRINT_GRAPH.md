# Sprint GRAPH — the unified graph sprint

**Status:** in-progress
**Branch:** `post-p5`
**Author:** Aaditya Srivathsan + Claude
**Started:** 2026-05-13

This sprint folds every architectural conclusion from the 2026-05-13 design conversation
into one branch. The conclusions span Glean's context graph, Ikigai's Large Graphical
Model, Palantir's Foundry Ontology, and our own composition reframe. The goal is to make
TraceMind's graph layer the **best graph layer of any local memory system** — multi-graph,
type-safe, composable, forecastable, and exportable into any AI conversation the user
opens.

Obsidian-style browsing (backlinks, wikilinks, force-directed view, markdown export)
remains intact — it is the *navigation surface* over what this sprint builds.

---

## 1. The five-layer graph stack

```
┌────────────────────────────────────────────────────────────────────┐
│  L5 — Composition Layer            (THIS SPRINT, new)              │
│  Graph algebra over Thread / View / Context subgraphs              │
│  Visual splice + attach-to-thread + cross-AI MCP export            │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L4 — Personal Ontology            (THIS SPRINT, new)              │
│  Object Types / Link Types / Action Types — versioned per user     │
│  Type-checks writes into kg_relations; inferred from data          │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L3 — Event / trajectory graph     (THIS SPRINT, new)              │
│  Per-context event_nodes / event_edges with frequency-floor        │
│  Cross-context bridges = separate sparse table, gated              │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L2 — Personal LGM (sparse PGM)    (THIS SPRINT, new crate tm-pgm) │
│  Random-variable nodes + conditional-dependence edges              │
│  Forecast / scenario / surprise — no NN required                   │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L1 — Entity + topic substrate     (SHIPPED, retained)             │
│  kg_entities, kg_relations, captured_signals, clusters, communities │
│  Memory Views (LM-11a..f), deny-list (LM-11/13), markdown export   │
└────────────────────────────────────────────────────────────────────┘
```

---

## 2. New SQL tables

All tables sit next to the existing schema in `memory.db`. Migrations are idempotent
(`CREATE TABLE IF NOT EXISTS`) and run inline at `GraphStore::open`.

### 2.1 `threads`

```sql
CREATE TABLE threads (
  id          TEXT PRIMARY KEY,         -- UUID v4
  context_id  TEXT,                     -- nullable; child of contexts.id
  title       TEXT NOT NULL DEFAULT '',
  source      TEXT NOT NULL,            -- 'claude_code' | 'goose' | 'tauri' | 'cli' | 'mcp' | 'other'
  started_at  TEXT NOT NULL,
  ended_at    TEXT,
  metadata    TEXT NOT NULL DEFAULT '{}'  -- JSON
);
```

A **thread** is one AI conversation. Every capture, query, commitment, and ingest
performed inside that conversation is tagged with `thread_id`. Threads are children of
contexts: a user can have many threads inside the "TraceMind" context.

### 2.2 `event_nodes` and `event_edges`

```sql
CREATE TABLE event_nodes (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,            -- 'capture'|'query'|'commitment'|'outcome'|'decision'|'ingest'
  ts          INTEGER NOT NULL,         -- unix millis
  payload_ref TEXT NOT NULL,            -- pointer ('captured_signals:123', 'traces:uuid', ...)
  cluster_id  INTEGER,
  context_id  TEXT,
  thread_id   TEXT,
  salience    REAL NOT NULL DEFAULT 0.0
);
CREATE INDEX idx_event_nodes_ts ON event_nodes(ts);
CREATE INDEX idx_event_nodes_thread ON event_nodes(thread_id);
CREATE INDEX idx_event_nodes_context ON event_nodes(context_id);

CREATE TABLE event_edges (
  id            TEXT PRIMARY KEY,
  from_id       TEXT NOT NULL,
  to_id         TEXT NOT NULL,
  kind          TEXT NOT NULL,          -- 'precedes'|'caused'|'co_occurs'|'resolves'|'contradicts'
  strength      REAL NOT NULL DEFAULT 0.0,
  support_count INTEGER NOT NULL DEFAULT 1,
  context_id    TEXT,                   -- scope: graph mining stays in-context
  first_seen    TEXT NOT NULL,
  last_seen     TEXT NOT NULL
);
CREATE INDEX idx_event_edges_from ON event_edges(from_id);
CREATE INDEX idx_event_edges_kind ON event_edges(kind, context_id);
```

`event_nodes` are stable references into the raw substrate. `event_edges` are mined
patterns — they only materialize when `support_count ≥ k` (Glean's frequency floor).
Default `k = 3` per context, configurable.

### 2.3 `bridge_edges`

```sql
CREATE TABLE bridge_edges (
  id            TEXT PRIMARY KEY,
  context_a     TEXT NOT NULL,
  context_b     TEXT NOT NULL,
  object_type_a TEXT,                   -- L4 ontology, if known
  object_type_b TEXT,
  evidence_kind TEXT NOT NULL,          -- 'shared_entity'|'shared_topic'|'user_confirmed'|'auto_strike'
  support_count INTEGER NOT NULL DEFAULT 1,
  status        TEXT NOT NULL DEFAULT 'proposed', -- 'proposed'|'accepted'|'denied'
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);
```

Sparse table that explicitly models cross-context bridges. Bridges are gated:
- positive feedback (`PositiveSignal`) bumps `status` toward `accepted`
- `wrong_context_suggestion` (3-strike, LM-12) demotes to `denied` and forwards to the
  existing `cross_ctx_block_list.json`
- ontology mismatch (when L4 says the two object types are incompatible) auto-denies

### 2.4 `ontology_object_types` and `ontology_link_types`

```sql
CREATE TABLE ontology_object_types (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  version         INTEGER NOT NULL DEFAULT 1,
  parent_id       TEXT,                  -- inheritance: fiction.location → location
  property_schema TEXT NOT NULL DEFAULT '{}',
  source          TEXT NOT NULL DEFAULT 'builtin',  -- 'builtin' | 'inferred' | 'user'
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);

CREATE TABLE ontology_link_types (
  id                  TEXT PRIMARY KEY,
  name                TEXT NOT NULL UNIQUE,
  version             INTEGER NOT NULL DEFAULT 1,
  from_object_type_id TEXT NOT NULL,
  to_object_type_id   TEXT NOT NULL,
  cardinality         TEXT NOT NULL DEFAULT 'many_to_many',
  source              TEXT NOT NULL DEFAULT 'builtin',
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);
```

Default ontology bootstraps with 12 object types: `Person`, `Organization`, `Project`,
`Concept`, `Technology`, `Decision`, `Event`, `Location`, `Artifact`, `Thread`, `Topic`,
`Commitment`. About 30 default link types cover most real interactions
(`WorksAt`, `MentionsTopic`, `PartOfProject`, `CommittedBy`, `ResolvesCommitment`, ...).

Type-checking happens at the write boundary: `GraphStore::upsert_relation` is wrapped to
verify `(from_object_type, predicate, to_object_type)` is allowed. Violators route to
`pending_relations` (LM-9) with a `type_check_failed` reason.

### 2.5 `lgm_variables` and `lgm_dependencies` (in `tm-pgm`)

```sql
CREATE TABLE lgm_variables (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  kind        TEXT NOT NULL,             -- 'bernoulli'|'categorical'|'gaussian'|'count'
  domain      TEXT NOT NULL DEFAULT '{}',
  observed_at TEXT NOT NULL
);

CREATE TABLE lgm_dependencies (
  child_id      TEXT NOT NULL,
  parent_id     TEXT NOT NULL,
  cpd           TEXT NOT NULL,           -- conditional-probability JSON
  log_score     REAL NOT NULL DEFAULT 0.0,
  support_count INTEGER NOT NULL DEFAULT 1,
  updated_at    TEXT NOT NULL,
  PRIMARY KEY (child_id, parent_id)
);
```

A tiny CPT-backed Bayesian network. Variables represent quantities we care about
(`commitment_completes`, `query_topic_t`, `active_context`, `weekday`, ...). Dependencies
encode conditional probability tables learned from `event_nodes`.

### 2.6 `memory_views_expression` (extension)

The existing `memory_views` table gets one additional column:

```sql
ALTER TABLE memory_views ADD COLUMN expression TEXT NOT NULL DEFAULT '';
```

If non-empty, `expression` is parsed as a graph-algebra expression and re-evaluated each
time the view is materialized. If empty, the existing flat-list semantics apply
(backwards compatible).

---

## 3. Code structure

### 3.1 `tm-graph` additions

```
crates/tm-graph/src/
├── algebra.rs                   ← NEW: graph-algebra ops
├── event_graph.rs               ← NEW: event_nodes/edges + materializer + promoter
├── thread_graph.rs              ← NEW: thread CRUD + subgraph materialization
├── bridge.rs                    ← NEW: cross-context bridge table CRUD
├── ontology_types.rs            ← NEW: Object/Link Type CRUD + type-check
├── portable_export.rs           ← NEW: graph → portable JSON
└── … (existing modules)
```

### 3.2 New crate `tm-pgm`

```
crates/tm-pgm/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── variable.rs              ← LGM variables
    ├── cpd.rs                   ← conditional-probability table
    ├── network.rs               ← bayes net + belief propagation
    └── learn.rs                 ← structure + parameter learning from event_nodes
```

### 3.3 Tauri commands

```
cmd_thread_list
cmd_thread_create
cmd_thread_end
cmd_thread_subgraph(thread_id)
cmd_compose(expr)                ← graph algebra expression
cmd_compose_attach(thread_id, view_id)
cmd_event_graph_explore(filter)
cmd_ontology_object_types()
cmd_ontology_link_types()
cmd_ontology_upsert_object_type(...)
cmd_ontology_upsert_link_type(...)
cmd_lgm_forecast(variable, evidence)
cmd_portable_export(view_id, path)
```

### 3.4 MCP additions

`tm-mcp::attach_view` — generalises LM-11d's per-query view parameter into a per-thread
default scope. The MCP server records `(client_thread_id → view_id)` and applies it on
every subsequent `memory_query` from that thread.

### 3.5 Frontend

```
crates/tm-tauri/ui/src/
├── views/
│   ├── ComposerView.tsx        ← side-by-side ThreadGraphs + set-op picker
│   ├── EventGraphView.tsx       ← timeline of mined event_edges
│   ├── OntologyView.tsx         ← Object Type / Link Type browser
│   └── ThreadsView.tsx          ← list + manage threads
└── api.ts                       ← (extend with new wrappers)
```

`GraphView` (existing force-directed Obsidian-style view) and `BacklinksPanel`
(LM-1) and `BriefView` are untouched. Backlinks, wikilinks, transclusion,
markdown export, and the entity drawer continue to work as before — they
operate against L1 substrate exactly the way they did pre-sprint.

---

## 4. Obsidian-style browsing — explicitly preserved

| Surface | Status |
|---|---|
| Backlinks panel (LM-1) | unchanged, still reads `kg_relations` |
| `[[wikilinks]]` (LM-2) | unchanged, `entity_resolve::resolve_in_text` |
| Entity drawer (LM-3, EntityDrawerView) | unchanged |
| Live graph update on capture (LM-4) | unchanged |
| Transclusion (LM-5a) | unchanged |
| Auto-tags (LM-5b) | unchanged |
| Daily notes (LM-5c) | unchanged |
| Memory Garden + community overlay (LM-20/22/23) | unchanged |
| Markdown export CLI (LM-16/17) | unchanged |
| Force-directed `GraphView` | unchanged |

The new Composer view sits *next to* the existing browsing — it is for *building* context
to send into another AI conversation, not for navigating notes. Both surfaces co-exist
under the same Tauri shell.

---

## 5. Acceptance criteria

- `cargo check --workspace` clean (no-default-features path)
- `cargo test -p tm-graph -p tm-pgm` green; new tests cover algebra, event-graph
  promotion, ontology type-check, thread subgraph extraction, LGM inference
- Tauri commands listed in §3.3 all return real payloads against a seeded DB (no stubs)
- `tracemind` CLI gains `thread`, `compose`, `ontology` subcommands
- Obsidian-style browsing surfaces verified intact (manual smoke + lint)
- `tsc --noEmit` + `vite build` clean
- Single squash-merge PR titled "feat(graph): unified Sprint GRAPH — composition layer +
  event graph + personal ontology + LGM"

---

## 6. Out-of-scope

- Force-directed re-layout of the Composer view (deferred — initial Composer uses
  small-multiples lists + a graph-algebra DSL textarea + summary cards)
- Full Palantir Action Types runtime (Object Types + Link Types ship; Actions are a
  separate Q3 item)
- Cross-device sync of any of the new tables (Sprint F territory)
- Visual drag-to-refine on the Composer (initial UI is select-and-compose; drag refine
  is a Q3 polish item)

