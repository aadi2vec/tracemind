# NLP-quality sprint — work plan

**Branch:** `claude/nlp-quality`
**Base:** `claude/tiered-promotion` (depends on the `EntityExtractor` trait shipped there)
**Goal:** Close the NLP-quality gap for the passive-capture path (clipboard, history, file import) without depending on the MCP structured-ingestion route.

## Framing

Ingest volume in TraceMind is weighted toward the **capture daemon + file import** paths, not MCP. That means:

- **LLM-in-the-loop NER (MCP structured ingestion)** only helps the minority of ingests that come through an LLM client. Pushed back.
- **On-device NER quality** is what moves the needle on the 80% passive volume. Prioritised.
- **Retrieval-side quality** (ColBERT reranking) lifts every query regardless of ingest path. First ticket.

## Ordered ticket list

### 1. TM-NLP-001 — ColBERT reranker always-on (no feature flag)

**Why first:** Biggest win with lowest risk. The code is already real (~459 lines of working MaxSim + ONNX). Feature-flagging it off is a habit, not an engineering reason.

**Scope:**
- Remove `colbert` feature from `crates/tm-rerank/Cargo.toml` — `ort`, `tokenizers`, `ndarray` become mandatory deps.
- Remove every `#[cfg(feature = "colbert")]` guard in `crates/tm-rerank/src/reranker.rs` and the "unavailable" fallback impl.
- Remove `colbert` feature from `crates/tm-retrieval/Cargo.toml` and the `#[cfg]` in `apply_colbert_maxsim`.
- Add `hf-hub = "0.4"` workspace dep. New `ColbertReranker::auto_download()` pulls `mixedbread-ai/mxbai-edge-colbert-v0-17m` to `~/.tracemind/models/mxbai-edge-colbert-v0-17m/` on first use.
- Wire `.with_reranker(…)` into `tm-cli` and `tm-mcp` binaries so every retrieval runs through ColBERT by default.
- Graceful fallback: if download fails (offline, rate-limited), log a warning and run without reranker (engine already handles `reranker: None`).
- Tests: existing MaxSim unit tests continue to pass. Add one test that `auto_download()` returns `Err` cleanly when the cache dir is unwritable.

**Acceptance:** `cargo test --workspace` green. `./target/release/tracemind query "..."` runs through ColBERT MaxSim with zero configuration.

### 2. TM-NLP-002 — Remove GLiNER scaffolding

**Why:** `crates/tm-ingest/src/gliner.rs` currently delegates every method to `HeuristicExtractor`. That's dishonest packaging. Ship only what's real.

**Scope:**
- Delete `crates/tm-ingest/src/gliner.rs`.
- Remove `gliner` feature + optional `ort`/`tokenizers`/`ndarray` deps from `crates/tm-ingest/Cargo.toml`.
- Remove the `gliner` module declaration from `crates/tm-ingest/src/lib.rs`.
- Update ARCHITECTURE.md: keep the `EntityExtractor` trait description (that's the real part), drop the "GlinerExtractor" claim, replace with a pointer to TM-NLP-004.

**Acceptance:** `cargo check --workspace` green. No reference to a "GlinerExtractor" anywhere in the tree.

### 3. TM-NLP-003 — Heuristic extractor upgrades (stdlib-only, no ONNX)

**Why:** Closes a real chunk of the heuristic → GLiNER gap on the passive-capture path without any model download. Each bullet is a small shippable improvement that plugs into the existing `EntityExtractor` trait.

**Scope (four sub-tickets, each merge-able independently):**

**3a. Case-insensitive entity linking to the growing graph.**
Right now `Rust` in the graph won't link to a new mention `rust`. The graph-dedup step already does case-insensitive lookup, but the *extractor* skips lowercase tokens entirely. Fix: after Title-Case pass, scan lowercase tokens against the existing graph's entity-name index; if match, emit as an entity. Zero inference cost, quality grows with graph size.

**3b. YAKE-style unsupervised keyphrase extraction.**
~50 LOC of stdlib-only code. Scores multi-word phrases by position + term-frequency + co-occurrence statistics. Gets `machine learning`, `vector search`, `large language model` even when lowercase. Threshold score, emit as `EntityType::Concept`. Expected recall bump on prose: +15–25 points.

**3c. Expanded relation patterns.**
Current `extract_triples` pattern list is short: `works_at`, `uses`, a few more. Add: `is a`, `are`, `has`, `have`, `contains`, `depends on`, `relies on`, `part of`, `built with`, `written in`, `runs on`. Wire to `Predicate::IsA`, `HasA`, `PartOf`, `DependsOn`. Each new pattern is a one-line regex + predicate mapping.

**3d. Fuzzy entity linking (Levenshtein ≤ 2).**
When an extracted entity name is within edit-distance 2 of an existing graph node ("Rustlang" → "Rust", "TypeScript" → "Typescript"), merge rather than create. Avoids graph fragmentation. Stdlib-only implementation is ~30 LOC.

**Acceptance per sub-ticket:** new unit tests covering the target cases; `cargo test -p tm-ingest` green; measurable entity-count delta on a small prose fixture.

### 4. TM-NLP-004 — Real GLiNER integration (validated)

**Why:** Last-mile quality for the passive-capture path. Delivers the ~85% F1 ceiling that stdlib heuristics fundamentally can't reach.

**Prerequisites before starting:**
- Local copy of `urchade/gliner_small-v2.1` ONNX model checked into a fixtures dir or accessible path.
- A small ground-truth fixture (20–50 sentences with gold entities) so the decoder can be unit-tested against real output, not just "does it compile".

**Scope:**
- Add `ort`, `tokenizers`, `ndarray`, `hf-hub` to `tm-ingest` as mandatory deps (no feature flag).
- Implement real span-based GLiNER decoding: tokenize `[CLS] label1 [SEP] label2 ... [SEP] text [SEP]`, run ONNX, decode span logits with NMS, map to `EntityType`.
- Auto-download on first use, cache to `~/.tracemind/models/gliner-small-v2.1/`.
- Fall back to `HeuristicExtractor` on model-load failure (offline, corrupt cache, etc.) — but the fallback is the ONLY reason falling back; there is no "scaffold" path.
- Benchmark on the fixture: report F1 vs. heuristic, commit the numbers.

**Do NOT start until a local model + fixture exists.** Writing the decoder blind is how silent-bug inference code ships.

### 5. TM-NLP-005 — MCP structured ingestion (TM-5.1-002 from main roadmap)

**Why deprioritised:** MCP is a minority of ingest traffic for this deployment. Still worth doing for the LLM-client path, but after the passive-path quality is lifted.

**Scope:** extend `memory_store` MCP tool to accept `{text, entities?, triples?}` with optional pre-extracted structure. If the client provides entities/triples, skip the extractor and go straight to graph upsert. Adds a compatibility bump to the MCP schema.

## Out of scope for this sprint

- Changes to retrieval ranking beyond enabling ColBERT.
- Changes to consolidation scheduling or clustering.
- MCP tool surface additions beyond `memory_store`.

## Architecture-doc follow-ups

After each ticket lands, update:
- `ARCHITECTURE.md` — tm-ingest + data-flow sections.
- `ROADMAP.md` — mark the ticket complete, move to "Completed Work".
- `CLAUDE.md` — if user-facing behaviour (env vars, CLI flags) changes.
