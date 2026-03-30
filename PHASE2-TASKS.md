# TraceMind Phase 2 — Task Board

Status: `TODO` | `IN_PROGRESS` | `DONE` | `BLOCKED`

---

## TM-P2-001 — Rewrite tm-graph with Kuzu
**Status:** TODO
**Branch:** `p2/kuzu-graph`
**Deps:** none
**What:** Replace rusqlite-backed GraphStore with Kuzu embedded graph DB.
Same public API: `open`, `upsert_entity`, `get_entity`, `find_entity_by_name`, `upsert_triple`, `get_triples_for_entity`, `k_hop_neighbors`.
**Accept:** `cargo test -p tm-graph` passes, k-hop uses Cypher not BFS.

---

## TM-P2-002 — Update upstream crates for new Embedder API
**Status:** TODO
**Branch:** `p2/embed-api`
**Deps:** TM-P2-001
**What:** `Embedder::new()` now returns `Result<Self>`. Update `tm-ingest`, `tm-retrieval`, `tm-mcp`, `tm-cli` to handle this. Tests use `Embedder::new_hash()`. VectorStore path is now a directory (LanceDB) not a `.vec` file.
**Accept:** `cargo build --workspace` compiles. `cargo test --workspace` passes.

---

## TM-P2-003 — End-to-end smoke test with real backends
**Status:** TODO
**Branch:** `p2/e2e-smoke`
**Deps:** TM-P2-002
**What:** `tracemind ingest` / `query` / `trace` / `status` work with Kuzu + LanceDB + fastembed hash embedder. Verify data persists across CLI calls.
**Accept:** 4-command smoke test passes on a temp data dir.

---

## TM-P2-004 — Real ONNX embeddings (fastembed model download)
**Status:** TODO
**Branch:** `p2/real-embed`
**Deps:** TM-P2-003
**What:** Wire `Embedder::new()` (real model) into CLI and MCP server. First run downloads ~80MB model. Add `--hash-embed` flag to CLI for offline/test use.
**Accept:** `tracemind ingest "hello"` uses real 384-dim embeddings. `tracemind query` returns semantically relevant results.

---

## TM-P2-005 — Procedural memory executor
**Status:** TODO
**Branch:** `p2/procedural`
**Deps:** TM-P2-002
**What:** Add `tm-procedural` crate (or module in tm-ingest). Store/retrieve Procedure+ProcedureStep in Kuzu via HAS_PROCEDURE edges. Dry-run executor (prints steps). Lifecycle: Active → Reinforced → Degraded → Deprecated.
**Accept:** Unit tests for store, lifecycle transitions, and dry-run execution.

---

## TM-P2-006 — Confidence decay background loop
**Status:** TODO
**Branch:** `p2/decay`
**Deps:** TM-P2-002
**What:** Add `tracemind decay` CLI command and a timer in tm-mcp that runs every 5 min. Decays entity/triple confidence by 0.95× per cycle. Entities below 0.05 are candidates for deletion (logged, not deleted in v1).
**Accept:** Unit test: decay 10 entities, verify confidence decreased. CLI command works.

---

## TM-P2-007 — Persistent bandit across all CLI commands
**Status:** TODO
**Branch:** `p2/bandit-persist`
**Deps:** TM-P2-002
**What:** Load bandit.json in RetrievalEngine. Save after every query (already done in CLI, now also in MCP). Add `--reset-bandit` flag.
**Accept:** Run 5 queries, `tracemind status` shows correct pull counts.

---

## TM-P2-008 — Tauri desktop app scaffold + IPC
**Status:** TODO
**Branch:** `p2/tauri-scaffold`
**Deps:** TM-P2-003
**What:** `cargo tauri init` in workspace. Tauri commands wrapping: ingest, query, trace, status, decay. React + TypeScript frontend shell with sidebar nav. No UI content yet — just the plumbing.
**Accept:** `cargo tauri dev` opens a window. Calling ingest from JS IPC returns entity count.

---

## TM-P2-009 — Tauri UI: query + dashboard
**Status:** TODO
**Branch:** `p2/tauri-ui`
**Deps:** TM-P2-008
**What:** Dashboard: entity count, triple count, trace count, bandit arm stats. Query view: text input, entity cards, triple list with names. Tailwind CSS.
**Accept:** Can ingest text and query it through the UI.

---

## TM-P2-010 — Tauri UI: memory timeline + entity graph
**Status:** TODO
**Branch:** `p2/tauri-graph`
**Deps:** TM-P2-009
**What:** Timeline view: chronological trace log. Entity explorer: browse entities, show connections. Graph visualization (D3 or Cytoscape.js) for entity neighborhood.
**Accept:** Can click an entity and see its neighborhood rendered as a graph.

---

## Execution order

```
P2-001 (kuzu)
  → P2-002 (api fix)
    → P2-003 (e2e smoke)
      → P2-004 (real embed)
      → P2-007 (bandit persist)
    → P2-005 (procedural)
    → P2-006 (decay)
  → P2-008 (tauri scaffold)
    → P2-009 (tauri query UI)
      → P2-010 (tauri graph UI)
```
