# TraceMind Phase 2 — Task Board

Status: `TODO` | `IN_PROGRESS` | `DONE` | `BLOCKED`

---

## TM-P2-001 — Graph store + tracing
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** none
**What:** Originally planned Kuzu rewrite, but Kuzu had fatal build issues (cxx version mismatch, cmake dependency). Decision: keep rusqlite, add tracing. Graph store works, 5 tests pass.
**Accept:** `cargo test -p tm-graph` passes. ✅

---

## TM-P2-002 — Update upstream crates for new Embedder API
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-001
**What:** `Embedder::new()` now returns `Result<Self>`. Updated `tm-ingest`, `tm-retrieval` to handle this. Tests use `Embedder::new_hash()` and temp dirs for LanceDB. VectorStore path is now a directory (LanceDB) not a `.vec` file.
**Accept:** `cargo build --workspace` compiles. `cargo test --workspace` passes (42/42). ✅

---

## TM-P2-003 — End-to-end smoke test with real backends
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** `tracemind ingest` / `query` / `trace` / `status` work with SQLite + LanceDB + fastembed hash embedder. Data persists across CLI calls. All 4 commands verified on temp data dir.
**Accept:** 4-command smoke test passes on a temp data dir. ✅

---

## TM-P2-004 — Real ONNX embeddings (fastembed model download)
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-003
**What:** `Embedder::new()` loads real all-MiniLM-L6-v2 via fastembed. Added `--hash-embed` global CLI flag and `TM_HASH_EMBED=1` env var for MCP. `IngestPipeline::open()` and `RetrievalEngine::open()` accept `hash_embed: bool`.
**Accept:** `tracemind ingest "hello"` uses real 384-dim embeddings. `--hash-embed` flag works. ✅

---

## TM-P2-005 — Procedural memory executor
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `ProcedureStore` (JSONL, dedup by id) and `dry_run()` to tm-episodic. CLI commands: `proc add`, `proc list`, `proc run` (dry-run), `proc feedback --success/--fail`. Lifecycle: Active → Reinforced → Degraded → Deprecated. 5 unit tests.
**Accept:** Unit tests pass. CLI `proc` commands work. ✅

---

## TM-P2-006 — Confidence decay
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `GraphStore::decay_all(factor, threshold)` method. `tracemind decay` CLI command with `--factor` and `--threshold` flags (defaults 0.95/0.05). Unit test passes. MCP timer deferred to Tauri phase.
**Accept:** Unit test passes. CLI `decay` command works. ✅

---

## TM-P2-007 — Persistent bandit across all CLI commands
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-002
**What:** Added `UcbBandit::load/save` to tm-controller. RetrievalEngine auto-loads/saves bandit.json. CLI Feedback/Status commands use load/save. Removed ~50 lines of duplicated persistence code from CLI.
**Accept:** Run 5 queries, `tracemind status` shows correct pull counts. ✅

---

## TM-P2-008 — Tauri desktop app scaffold + IPC
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-003
**What:** `cargo tauri init` in workspace. Tauri commands wrapping: ingest, query, trace, status, decay. React + TypeScript frontend shell with sidebar nav. No UI content yet — just the plumbing.
**Accept:** `cargo tauri dev` opens a window. Calling ingest from JS IPC returns entity count.

---

## TM-P2-009 — Tauri UI: query + dashboard
**Status:** DONE
**Branch:** `claude/crazy-spence`
**Deps:** TM-P2-008
**What:** Dashboard: entity count, triple count, trace count, bandit arm stats. Query view: text input, entity cards, triple list with names. Tailwind CSS.
**Accept:** Can ingest text and query it through the UI.

---

## TM-P2-010 — Tauri UI: memory timeline + entity graph
**Status:** TODO (graph viz not yet implemented)
**Branch:** `p2/tauri-graph`
**Deps:** TM-P2-009
**What:** Timeline view: chronological trace log. Entity explorer: browse entities, show connections. Graph visualization (D3 or Cytoscape.js) for entity neighborhood.
**Accept:** Can click an entity and see its neighborhood rendered as a graph.

---

## Execution order

```
P2-001 (graph+tracing) ✅
  → P2-002 (api fix) ✅
    → P2-003 (e2e smoke) ✅
      → P2-004 (real embed) ✅
      → P2-007 (bandit persist) ✅
    → P2-005 (procedural) ✅
    → P2-006 (decay) ✅
  → P2-008 (tauri scaffold) ✅
    → P2-009 (tauri query UI) ✅
      → P2-010 (tauri graph viz) — TODO
```
