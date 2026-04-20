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

### 4. TM-NLP-004 — Real GLiNER integration (validated) — ✅ SHIPPED

**Shipped 2026-04-19.** Real span-based zero-shot NER now runs by default on every ingest path.

**What landed:**
- `crates/tm-ingest/src/gliner.rs` (~400 LOC) — real `GlinerExtractor`:
  - `ort` (=2.0.0-rc.10) session over `onnx-community/gliner_small-v2.1` (int8, 183 MB).
  - `hf-hub` (0.4) auto-download on first use, cache to `~/.cache/huggingface/hub/` (shared with ColBERT).
  - DeBERTa-v3 tokenizer via `tokenizers` (0.21) with `<<ENT>>` (128002) / `<<SEP>>` (128003) markers.
  - 6-input ONNX signature: `input_ids`, `attention_mask`, `words_mask`, `text_lengths`, `span_idx`, `span_mask` — dense `[B, num_words × MAX_WIDTH, 2]` grid with a mask rather than compacted spans (that was the shape the exported model actually expected).
  - 8-label default: `person / organization / location / technology / product / project / concept / event`.
  - Decoder: sigmoid → threshold 0.3 → greedy overlap NMS → dedup by name+type.
- `auto_download_default()` → `Option<Self>`; silently returns `None` on any hub / model / tokenizer failure. Every binary wires it the same way:
  ```rust
  if let Some(gli) = GlinerExtractor::auto_download_default() {
      pipeline = pipeline.with_extractor(Box::new(gli));
  }
  ```
  Call sites updated: `tm-cli` (query + import), `tm-mcp`, `tm-tauri`, `tm-capture` (clipboard_loop, history_loop, consolidation_loop, priority_consolidation_loop).
- **Silent dedup bug fixed in `IngestPipeline::decide_memory_op`:** the old code decided Add vs Update vs Noop purely on embedding similarity of `"name: full_text"`, which meant a 4-entity sentence collapsed into a single Update of the first entity (all four embedded near-identically). Added a `names_look_like_same_entity` guard requiring name-substring agreement (≥ 4 chars) before Update/Noop. Without this fix, the e2e hit rate was 7/10; with it, 10/10.
- Eval harnesses in `tm-bench`:
  - `tm-bench-ner` — P/R/F1 on `crates/tm-bench/fixtures/ner_eval.jsonl` (30 labeled sentences), supports `--heuristic-only` / `--gliner-only`.
  - `tm-bench-ner-e2e` — spins up a fresh temp DB, ingests the full fixture through the real pipeline, then runs 10 natural-language probe queries and asserts ≥ 80 % of them surface an expected entity in top-k.

**Measured on the 30-sentence fixture:**

| extractor | P | R | F1 | latency |
|-----------|---|---|----|---------|
| `HeuristicExtractor` | 0.322 | 0.961 | **0.483** | <1 ms/doc |
| `GlinerExtractor` @ 0.3 | 0.928 | 0.882 | **0.905** | ~10 ms/doc inference, ~44 ms/doc full-pipeline ingest |

Threshold sweep picked 0.3 (0.4 → 0.844, 0.5 → 0.795).

**E2E probe result: 10/10 = 100 %** on the 10 natural-language queries in `ner_e2e.rs` (who works at Anthropic, what is TraceMind built with, Apple hardware announcements, etc.).

**Bundle cost:** 183 MB int8 ONNX + tokenizer/config, fetched once per machine, shared with ColBERT's HF cache. Acceptable for a .dmg-style distribution.

**Acceptance met:** `cargo test -p tm-ingest` 44/44 green, `cargo run --bin tm-bench-ner-e2e` 10/10 green, F1 numbers committed above.

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
