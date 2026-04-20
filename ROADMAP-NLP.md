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

### 5. TM-NLP-005 — Bundled / offline-first model loading — ✅ SHIPPED

**Shipped 2026-04-19.** Packaged binaries run with zero outbound HuggingFace traffic.

**What landed:**
- `tm_types::bundled::init()` — single-call resolver that walks
  `$TM_MODELS_DIR` → exe-relative bundle (`<exe>/../Resources/models` for
  the Tauri `.app`) → `~/.tracemind/models` → hf-hub default, and sets
  `HF_HOME` + `FASTEMBED_CACHE_DIR` so `hf-hub` (GLiNER, ColBERT) and
  `fastembed` (BGE) all read bundled weights. Honours user-set env vars
  (no clobber).
- Wired into every binary's `main()`: `tracemind`, `tm-mcp`, `tm-tauri`,
  `tm-capture`, `tm-bench`.
- `crates/tm-tauri/tauri.conf.json` — `bundle.resources` globs
  `models/**/*` into `Contents/Resources/models/`.
- `scripts/fetch-models.sh` — pre-populates the HF cache layout at build
  time (GLiNER, mxbai-edge-colbert, BGE); intended for
  `beforeBundleCommand`.
- `THIRD_PARTY_LICENSES.md` — GLiNER Apache-2.0, mxbai-edge-colbert
  Apache-2.0, BGE MIT.

**Size budget:** ~347 MB of int8 ONNX → ~200–250 MB compressed `.dmg`,
comparable to Slack/Discord/VS Code.

**Privacy guarantee:** when `init()` resolves a populated bundle, **no
TraceMind binary opens a connection to HuggingFace at any point.** The
`auto_download_or_none` paths still exist but only run when the bundle is
missing a specific model.

**Acceptance met:** `cargo test -p tm-types` 24/24 green (4 new
`bundled::` tests); smoke:
`TM_MODELS_DIR=/tmp/x tracemind status` → `"[tracemind] using bundled
models from /tmp/x (TM_MODELS_DIR)"`.

---

## Quality gaps — next-up backlog

After TM-NLP-001/002/003/004/005, these are the remaining holes, ranked
by impact. Each gets its own ticket; sequencing reflects dependency + ROI.

### 6. TM-NLP-006 — GLiREL relation extraction

**Why:** the biggest remaining NLP hole. GLiNER is NER-only; edges in the
graph are still surface-pattern regex (`works_at`, `uses`, …). Multi-hop
reasoning collapses when edges are sparse or mislabelled.

**Scope:**
- Add a `RelationExtractor` trait (symmetric to `EntityExtractor`).
- Ship `GlirelExtractor` over `knowledgator/gliner-multitask-large-v1.0`
  or `jackboyla/glirel` ONNX export (same `ort` + `hf-hub` pattern as
  GLiNER).
- Zero-shot relation set, extendable via trait constructor; defaults:
  `works_at`, `founded`, `part_of`, `uses`, `built_with`, `located_in`,
  `collaborates_with`, `causes`.
- Wire into `IngestPipeline::with_relation_extractor`.
- New `tm-bench-rel` harness on a labeled relation fixture; target F1 ≥
  0.70 (current regex baseline ≈ 0.35 on the same fixture).
- Fall back to the heuristic patterns on model-load failure.

**Impact:** High — graph edges become trustworthy, unlocks Phase 5
reasoning chains that currently surface only because the node set is
good enough.

### 7. TM-NLP-007 — Coreference resolution

**Why:** "Apple announced the M4. It has 40 % more cores." — "It" never
links to M4, so that fact is a silently orphaned triple. Every pronoun in
a multi-sentence capture is a lost edge.

**Scope:**
- Phase A (stdlib, cheap): deterministic pronoun resolver. Same-paragraph
  back-reference from {it / they / he / she / this / that} to the most
  recent matching entity by type + number agreement. Covers ~60 % of cases
  with zero model cost; runs before relation extraction.
- Phase B (optional): integrate `allenai/longformer-scico` or a small
  ONNX-exported coreference model for cross-paragraph / cross-document
  chains. Gated on Phase A not being enough in the eval harness.
- Add a `coref_chains` field to `IngestResult` so downstream stages can
  rewrite pronouns → canonical names before relation extraction.
- Eval: extend `fixtures/ner_eval.jsonl` with 30 sentences containing
  antecedent/pronoun pairs; target 80 % of pronouns correctly resolved.

**Impact:** High — fixes a bug-class of missing edges across almost every
multi-sentence capture.

### 8. TM-NLP-008 — Entity canonicalization

**Why:** `"Dario"`, `"Dario Amodei"`, `"Anthropic CEO"` stay as three
separate nodes today. TM-NLP-003d (Levenshtein ≤ 2) catches typos but not
semantic aliases. The graph fragments linearly over time.

**Scope:**
- New `Canonicalizer` that runs after ingest, batched (not per-sentence):
  1. For each entity, compute an alias vector = BGE embedding of
     `"<name> (<type>)"`.
  2. Agglomerative cluster with cosine ≥ 0.90 threshold within-type.
  3. For each cluster, pick the longest name as canonical and rewrite all
     incoming/outgoing edges.
  4. Keep the alias list on the canonical node for display + future match.
- Run on every consolidation pass (no extra cost since embeddings already
  exist).
- Optional: manual override file
  `~/.tracemind/aliases.toml` for hand-curated merges/splits.
- Eval: synthetic fixture with 50 aliased entity sets; target ≥ 85 %
  correct cluster assignment, ≤ 5 % false merges.

**Impact:** Medium-high — graph stays compact over months of use instead
of fragmenting.

### 9. TM-UX-001 — Seamless capture + recommendation

**Why:** the pipeline is capture-heavy and recall-reactive. Users have to
ask a query to get value out. "Seamless" means (a) the user trusts the
capture is happening (feedback), and (b) recall surfaces more than the
literal top-k — adjacent memories the user didn't know to ask for.

**Scope (landing in phases):**
- **Phase A — Recommendation — ✅ SHIPPED (2026-04-20).** Every query
  response now carries `related_entities` — 1-hop graph neighbours of the
  top-k direct hits, filtered against the primary set, scored by
  `cosine(query, candidate_embedding)` with a small graph-proximity floor
  so structural neighbours still surface when vector recall is thin.
  Surfaced via CLI (`tracemind query "…"` prints a `Related:` block with
  reason attribution) and MCP (`memory_query` grows a `related_entities`
  field with `{id, name, type, score, reason}`). Bounded fan-out
  (top-5 seeds × 1 hop, top-5 returned). All three retrieval paths
  (bandit, decomposed, temporal) populate the field. Unit test
  `related_entities_surfaces_one_hop_neighbours` in `tm-retrieval` covers
  the happy path + dedup guarantee + empty-primary case. See
  `crates/tm-retrieval/src/engine.rs::compute_related_entities`.
- **Phase B — Capture feedback — ✅ SHIPPED (2026-04-20).** Capture
  daemon and MCP `memory_store` now emit a bounded JSONL ring to
  `~/.tracemind/recent.jsonl` (last 100 events, rewrite-on-append) with
  `{timestamp, source, tier, content_hash, skipped_reason, promoted,
  text_preview}`. `RecentCapture` lives in `tm-types::recent`; `RecentStore`
  (open/append/read_all/recent) lives in `tm-episodic::recent_store`.
  Sources wired: clipboard + shell-history loops in `tm-capture`, and
  `handle_memory_store` in `tm-mcp`. Surfaced via
  `tracemind recent [--limit N] [--json]` — a newest-first table with
  promoted/skipped status per event. Unit tests in
  `tm-episodic::recent_store` cover ring-cap-at-capacity (wraparound),
  newest-first read, missing-file tolerance, and malformed-line skip.
  Tauri tray ticker deferred to Phase B2 — the ring file is the shared
  contract so any later UI can consume it without code changes.
- **Phase C (separate ticket):** proactive surfacing. When an MCP client
  sends a tool call, `memory_query` gets auto-invoked on the query-derived
  context even without an explicit ask — returns top-3 memories as a
  system note.

**Impact:** High — shifts TraceMind from "memory you query" to "memory
that surfaces itself", which is the product differentiator vs Honcho.

### 10. TM-QUAL-001 — Continuous retrieval eval

**Why:** `tm-bench-ner-e2e` covers NER → retrieval but there's no CI-gated
guard against quality regression on the query side. Quality quietly
erodes between releases.

**Scope:**
- Extend `tm-bench-ner-e2e` to a 100-query fixture covering the 8 entity
  types + temporal queries + multi-hop.
- Add `tm-bench-retrieval` binary that reports MRR@10, NDCG@10, top-1
  accuracy against the fixture.
- GitHub Actions workflow runs it on every PR; fails if MRR drops by >
  0.05 vs main.
- Publish last-10-runs results to `docs/quality.md` (auto-committed).

**Impact:** Medium — infrastructural, not user-visible, but unlocks
aggressive iteration on the other quality gaps without fear of silent
regression.

### 11. TM-NLP-010 — Better embeddings for technical/code content

**Why:** BGE-small is general-purpose. Much of TraceMind's capture volume
is code, IDE content, and technical prose — embedded poorly compared to a
domain-tuned model.

**Scope:**
- Add `EmbedModel::NomicV15` (`nomic-ai/nomic-embed-text-v1.5`) as an
  option in `tm-vector`. 137 M params, still ONNX-exportable, 768-dim.
- Add `EmbedModel::JinaCode` (`jinaai/jina-embeddings-v2-base-code`) as a
  content-type-gated alternative for captures classified as code.
- Consolidator re-embeds existing entities on model switch (gated behind
  `TM_EMBED_MIGRATE=1`).
- Eval: re-run retrieval eval (TM-QUAL-001) on technical fixture; pick
  whichever model wins.

**Impact:** Medium — lifts retrieval on the dominant capture type.

### 12. TM-GOV-001 — PII filter upgrades

**Why:** current governance is regex-only; misses international phone
formats, address variants, contextual PII ("my SSN is nine digits no
dashes"). Becomes blocking before multi-user.

**Scope:**
- Add locale-aware phone patterns (E.164 + regional).
- Address detection (structured: postal-code + street-suffix co-occurrence).
- Context classifier over GLiNER: if a `Person` entity co-occurs within 5
  tokens of a digit run of length ≥ 7, redact unless explicitly exempted.
- Redaction audit log: every governance decision recorded to
  `~/.tracemind/governance.log` with text-hash (not plaintext).

**Impact:** Medium — not a quality-of-recall issue, but a correctness
issue that will bite multi-user ACL work in Phase 6.

### 13. TM-NLP-011 — Consolidation super-entities

**Why:** Ebbinghaus decay is wired but actual super-entity summarisation
(Phase 5 goal) isn't. Graph grows linearly; clusters of near-duplicate
entities accumulate.

**Scope:**
- Clustering pass in `consolidate_normal` that groups entities with
  cosine ≥ 0.85 and shared graph neighbours; creates a `super_entity`
  node with merged description + child-entity list.
- Super-entities are recallable alongside leaves, ranked by cluster
  coverage; query cost-amortisation for popular topics.
- Migration: `tm-cli consolidate --super` runs the pass on existing
  graphs.

**Impact:** Medium — scales graph size sub-linearly over months.

### 14. TM-NLP-012 — Query-time rewriting / synonym expansion

**Why:** typos in the query route poorly; "macbook" misses
"MacBook Pro" as an entity if the graph canonicalization hasn't caught
it. No synonym expansion ("JS" → "JavaScript").

**Scope:**
- Pre-query pass: lowercase-match the query tokens against graph entity
  names; if a close match (Levenshtein ≤ 2 or embedding cosine ≥ 0.85)
  exists, inject the canonical form as an expanded query alongside the
  original.
- Synonym file `~/.tracemind/synonyms.toml` for hand-curated acronym
  expansions.
- Log query rewrites to the trace stream for auditability.

**Impact:** Low-medium — easy wins on a long tail of queries; depends on
TM-NLP-008 (canonicalization) to really shine.

### 15. TM-NLP-009 — MCP structured ingestion (was TM-NLP-005 in the pre-bundling plan)

**Why deprioritised:** MCP is a minority of ingest traffic for this
deployment. Still worth doing for the LLM-client path, but after the
passive-path quality is lifted.

**Scope:** extend `memory_store` MCP tool to accept
`{text, entities?, triples?}` with optional pre-extracted structure. If
the client provides entities/triples, skip the extractor and go straight
to graph upsert. Adds a compatibility bump to the MCP schema.

**Impact:** Low — marginal improvement on the MCP path; large if the
majority of traffic ever shifts there (unlikely given product direction).

## Out of scope for this sprint

- Changes to consolidation scheduling itself (tracked in Phase 5).
- JEPA / world-model work (Phase 5+).
- Multi-user ACL / REST API (Phase 6).

## Architecture-doc follow-ups

After each ticket lands, update:
- `ARCHITECTURE.md` — tm-ingest + data-flow sections.
- `ROADMAP.md` — mark the ticket complete, move to "Completed Work".
- `CLAUDE.md` — if user-facing behaviour (env vars, CLI flags) changes.
