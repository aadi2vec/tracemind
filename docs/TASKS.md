# TraceMind — Task List

**Consolidated**: 2026-05-09. Canonical task list for all implementation work.

Legend: `[x]` done, `[-]` in progress / partial, `[ ]` not started. Est. = estimated F1 lift where applicable.

---

## Shipped (reference)

- [x] Phase 4 / Sprint C-1: bitemporal substrate (tm-temporal embedded into tm-graph; query_at / history; entities + triples backfilled)
- [x] Phase 4 / Sprint C-2: TMS-backed confidence (BeliefStore wraps tm-tms; contradictions surfaced in brief; effective_confidence policy)
- [x] 24-crate Rust workspace compiles and tests pass
- [x] Intent arc types: Need, Sentiment, Action + Belief trait (`tm-intent`)
- [x] Intent arc persistence: SQLite tables + store methods for needs/sentiments/actions
- [x] MCP tools: memory_need, memory_sentiment, memory_action, memory_arc
- [x] CLI commands: need, sentiment, action, arc
- [x] Commitment primitive + state machine + outcome attachment
- [x] CommitmentMiner (implicit phrase mining) + candidate confirm/dismiss
- [x] Daily brief (BriefBuilder) + insights + pattern detector
- [x] World model v0 (f_outcome logistic regression, preflight)
- [x] Outcome proposals (implicit text matcher → accept/dismiss)
- [x] Tier-0 extractive answerer (always-on fallback)
- [x] Tier-1 scaffolded (LocalLlmBackend, Qwen 2.5 1.5B Q4, llama-cpp-2)
- [x] Tier-2 scaffolded (AppleFmBackend stub)
- [x] ColBERT rerank (mxbai-edge-colbert, auto-download)
- [x] BGE-small ONNX embeddings (384-dim)
- [x] 5-arm LinUCB bandit retrieval
- [x] Reasoning: chains, analogy (WL kernel), causal trace, consolidator
- [x] Capture daemon (clipboard + shell history)
- [x] Tauri desktop app shell (basic query/ingest)
- [x] LoCoMo benchmark harness (mini-set, 20 questions)
- [x] LoCoMo v0.2 fixes: speaker-prefix strip, Q→A adjacency, skip-question fallback
- [x] LoCoMo quick wins applied: cosine threshold 0.4→0.2, Q→A window 1→3 turns
- [x] tm-temporal crate (bitemporal facts, TemporalStore, 10 tests)
- [x] tm-tms crate (JTMS engine, BFS propagation, 11 tests)
- [x] tm-modal crate (types, encoder trait, co-occurrence detector, 13 tests)
- [x] tm-engram crate (Engram SDK scaffold, 4 tests)

---

## Priority 0 — Recordable demo (gates everything else)

Sprint C-2 unlocked the *retraction beat* (the demo's hook). Now ship a clean recordable demo before any further engine work. Items are sequential — do not parallelize without explicit redirect.

- [x] **D-1: 3-minute "day in the life" script** — `docs/DEMO_SCRIPT.md`. Five shots; retraction beat in shot 2. Pre-roll uses `tracemind demo restore` + `tracemind demo preroll`.
- [x] **D-2: Pre-warmed demo fixture** — `tracemind demo restore`. ~15 entities, 16 triples, 1 contradiction, 4 open + 5 resolved commitments, deterministic UUIDv5 from a frozen namespace. Sidecar persistence (`memory.db.contradictions.json`) so the contradiction survives across CLI invocations.
- [x] **D-3: Real ambient capture in demo path** — `tracemind demo preroll [--seconds N]` spawns the existing `tracemind-capture` daemon silently (stdout/stderr suppressed) for the pre-roll window, then reaps it. Resolves the binary via `$TM_CAPTURE_BIN` → sibling executable → PATH.
- [x] **D-4: Brief renders in the Tauri app** — new `cmd_brief` IPC + `BriefView.tsx` render the same DailyBrief the CLI shows, with the contradictions row at the top. `Brief` is the new default landing tab. The `models/**/*` glob is satisfied by the existing `models/manifest.json` placeholder.
- [x] **D-5: Single-binary install** — `scripts/install.sh`. Detects platform, downloads release tarball, optional SHA256 verification, installs `tracemind` / `tm-mcp` / `tracemind-capture` to `/usr/local/bin` (or `~/.local/bin`), creates `~/.tracemind/`. Idempotent.
- [x] **D-6: One-screen product close** — `docs/PRODUCT_CLOSE.md`. Three products / one engine: TraceMind (personal memory OS), Engram (memory SDK), Rosetta (semantic code memory). Shared crates listed; install one-liner on the end card.

**Why P0:** items 1–4 unlock the recorded demo. 5–6 are needed before screen-sharing to anyone outside.

---

## Priority 1 — Quality (highest F1 impact, pure code work)

### LoCoMo Tier-0 remaining fixes

- [ ] **Heuristic NER for span extraction** — dates, money, named entities, percentages. Return the span ("April 20", "$2.5M") instead of the full sentence. (Est. +8–15 F1)
- [ ] **Yes/no oracle** — pre-screen `Did/Was/Is/Has` questions; if predicate disagrees with retrieved evidence, emit "No" + the contradicting fact. Tier-0 extractive cannot do this today. (Est. +5–10 F1)
- [ ] **Recency bias for duplicate-entity turns** — when multiple turns mention the same entity, prefer the latest mention. (Est. +3–5 F1)

### Tier-1 as default

- [ ] **Make Tier-1 the shipping default** — today it only activates with `--features local-llm`. Needs: first-run download flow with progress UI, auto-download on first query when weights missing, Tier-1 dispatched for all synthesis (not just structured tasks).
- [ ] **Run LoCoMo mini-set with Tier-1** — measure actual F1 gain from LLM synthesis vs. extractive. Gate: ≥60 F1.

---

## Priority 2 — Wire isolated crates into the pipeline

### Wire tm-tms into ingest + retrieval (Sprint C-2 — mostly shipped)

- [x] Call `TmsEngine.assert_belief()` at ingest time when entities/triples are upserted
- [x] Contradiction detection at ingest (cosine < -0.8 via tm-tms threshold; schema/temporal triggers still pending)
- [x] Belief-aware retrieval ranking: hide `Out`, downrank `Contradicted` via `effective_confidence`
- [x] Surface contradictions in daily brief via tm-reflect
- [ ] Schema-constraint and temporal-overlap contradiction triggers (in addition to cosine)
- [ ] Persist TMS state across restarts (currently rebuilt from live triples)

### Wire tm-temporal into tm-graph (Sprint C-1 — shipped)

- [x] Bitemporal substrate via embedded `TemporalStore` (sibling DB); write-through on every entity/triple upsert
- [x] `GraphStore::entity_at`, `triple_at`, `entity_history`, `triple_history`
- [ ] `belief_revisions` table for explicit retraction provenance (separate from JTMS retractions)
- [ ] Time-machine queries in CLI + MCP: "what was I thinking in March?"
- [ ] `GraphStore::diff(from, to)` for change inspection

---

## Priority 3 — Product (session quality + UX)

### WorkingMemory ring buffer

- [ ] Define `tm-types::WorkingMemory` — Vec-backed in-RAM ring of last N turns + retrieval results
- [ ] Feed working memory into subsequent queries (session context for follow-ups)
- [ ] Include working memory in `ContextSnapshot` at commitment time

### NarrativeResponse

- [ ] Define `NarrativeResponse` in tm-types: `text`, `citations`, `related_threads`, optional `surprise`, optional `voice_audio`
- [ ] Replace raw `RetrievalResult` / `AnswerResponse` at all user-facing boundaries (CLI, MCP, Tauri)

### Query rewriting

- [ ] Tier-1 paraphrase expansion: expand raw question into 3 paraphrases, run all through retrieval, RRA fusion. (Est. +5–8 F1 on multi-hop)

### Auto-detection pipeline components

- [ ] `NeedDetector` in tm-capture — mine "I need to", "problem is", "goal is" patterns from capture stream
- [ ] `SentimentScorer` — heuristic valence scoring from captured text (keyword-based first)
- [ ] `ActionMatcher` — match shell commands, file edits, git commits to open commitments via embedding similarity

---

## Priority 4 — Tauri UI surfaces

- [ ] Daily brief panel (read on open, dismissable, archived)
- [ ] Commitment timeline (vertical, color-coded by state, click → drawer with full context)
- [ ] Intent arc visualization (Need → Sentiment → Commitment → Action → Outcome graph)
- [ ] Memory garden (force-directed entity graph)
- [ ] Capture timeline (chronological ingest view)
- [ ] "What I noticed" surprise panel
- [ ] First-run onboarding (sample data → meaningful brief in 60 seconds)
- [ ] Settings panel (personality, voice, brief schedule)
- [ ] Calibration panel (predictions made, outcome accuracy, Brier score, pattern stats)

---

## Priority 5 — Voice + capture moat

- [ ] `tm-voice` crate — Whisper-tiny STT (39MB) + Piper TTS (60MB)
- [ ] Global hotkey ⌘⇧Space → hold-to-record → Tier-1 normalizes to Commitment draft
- [ ] Daily brief TTS playback
- [ ] Global hotkey ⌘⇧M for quick capture
- [ ] Screenshot capture + VLM caption (SigLIP-small for encoding, Moondream/Phi-3.5-vision for caption)
- [ ] Obsidian vault import (walk all Markdown files, ingest)
- [ ] Browser extension (capture active tab content)

---

## Priority 6 — Engine depth (Phase 5A)

### Cross-modal pipeline

- [ ] Implement SigLIP-small ONNX encoder for images
- [ ] Implement tree-sitter + BGE code encoder
- [ ] Wire `ModalIngestPipeline` into tm-ingest (co-occurrence edges within 30-second window)
- [ ] Add arm 5 (cross-modal) to tm-controller: top_k=12, hops=1, cross-modal=yes
- [ ] Cross-modal chains in tm-reason (`CrossModalChainBuilder`)
- [ ] Cross-modal citations in synthesis (screenshots + code spans)

### World model upgrades

- [ ] `f_topic` MLP (2-layer, 384→512→384, InfoNCE) — powers L1 silent prefetch
- [ ] L1 silent prefetch: background task pre-warms tm-retrieval based on f_topic predictions
- [ ] Platt scaling / isotonic regression on f_outcome for calibrated probabilities
- [ ] Extend LinUCB context vector with working-memory state, tier, affective-graph density

### Nightly processes

- [ ] `tm-reflect` nightly cron/daemon (scheduled brief generation)
- [ ] TMS consistency check nightly
- [ ] Deductive/inductive promotion (raw captures → graph entities)
- [ ] Temporal GC (archive retracted facts older than N days)
- [ ] Two-speed ingestion: signal lake (fast, raw) + graph promotion (background)

---

## Priority 7 — Engram extraction (Phase 5B)

- [ ] Finalize Belief trait API across all intent-arc types
- [ ] `tm-engram` full implementation: assert/retract/world_at/contradictions/history_of/set_goal/record_action/observe
- [ ] MCP tools: memory_believe, memory_retract, memory_world_at, memory_contradictions
- [ ] Standalone Engram MCP server binary
- [ ] Python wrapper (PyO3) → PyPI: `engram`
- [ ] TypeScript wrapper (napi-rs) → npm: `@tracemind/engram`
- [ ] Integration test with Claude Code
- [ ] Publish to crates.io
- [ ] Benchmark: assertion latency <1ms, retraction propagation <10ms for 10k beliefs
- [ ] 3 example agents (research assistant, code review, customer support)

---

## Priority 8 — Rosetta foundation (Phase 5C)

- [ ] `tm-semcode` crate: tree-sitter parsing (Rust, Python, TypeScript, Go, Java)
- [ ] Intent extraction pipeline: code → AST analysis → cross-modal fusion with tests/docs/git/PRs → CodeIntent
- [ ] `SemanticDiff` engine: semantic change classification (Cosmetic/Refactor/IntentShift/New/Deleted)
- [ ] Intent drift detection (bitemporal over git history)
- [ ] `tm-rosetta` CLI: `rosetta diff HEAD~5..HEAD`
- [ ] MCP tools: code_intent, code_semantic_diff
- [ ] VS Code extension stub
- [ ] (Stretch) Intent-preserving refactoring generation + property-based test generation

---

## Priority 9 — Mobile + sync (Phase 6)

- [ ] UniFFI bindings (Swift + Kotlin)
- [ ] iOS app (SwiftUI)
- [ ] Android app (Compose)
- [ ] Apple FoundationModels backend (macOS 26+)
- [ ] Photo ingest (EXIF + VLM caption)
- [ ] `tm-sync` crate: Automerge CRDTs over iCloud/Drive
- [ ] End-to-end encryption with per-user keypair
- [ ] Opt-in encrypted-cloud Tier (low-end devices only)

---

## Priority 10 — Scale + world model v2 (Phase 7)

- [ ] `f_outcome` v2: 4-layer transformer (~3M params, d=256, 4-class polarity)
- [ ] HNSW vector index for 100k+ entities (replace flat scan)
- [ ] Mamba/SSM history compression
- [ ] GraphSAGE GNN for AnalogySolver (replace WL kernel)
- [ ] Louvain/Leiden community detection in Consolidator
- [ ] Factorization machine / MLP for PatternDetector (cross-cell interactions)
- [ ] `tm-preference` crate: triplet contrastive loss on (query, kept_answer, rejected_answer)
- [ ] Loop 4 counterfactual replay (doubly-robust OPE over trajectory store)
- [ ] Iterative/agentic retrieval (multi-step query refinement)
- [ ] Context-budget allocator (dynamic retrieval budget per query complexity)

---

## Evaluation infrastructure

- [ ] Full LoCoMo run (~7,000 questions) — mini-set numbers not comparable to competitors
- [ ] Cross-modal eval harness: 50 hand-labeled multi-modal reasoning scenarios
- [ ] Bitemporal correctness test suite (formal, 100%)
- [ ] Intent preservation eval: 100 labeled before/after refactoring pairs
- [ ] Belief consistency eval: formal JTMS spec compliance
- [ ] Performance benchmarking: idle RAM, active RAM, cold-query latency, Tier-1 hot-query latency

---

## Priority 11 — LLM packaging + on-device personalization (deferred)

Lower-priority phase queued behind the recordable demo and the existing P1–P10 work. Two layers:

### L1 — `tm-llm` packaging crate

- [ ] New `tm-llm` crate: `ModelManifest` (sha256, size, prompt template, tokenizer hash, license) + `ModelRegistry` reading `~/.tracemind/models/manifest.toml`
- [ ] First-run model fetch with checksum verification + atomic install (no half-downloaded weights)
- [ ] LoRA adapter slot: `BaseModel + Vec<AdapterSpec>` with hot-swap at the `LocalLlmBackend` boundary
- [ ] CLI surface: `tracemind models list / install / remove / verify`
- [ ] MCP surface: `model_status` returning manifest + adapter state

### L2 — Resource-constrained on-device finetune

- [ ] Python sidecar (`tools/finetune/`) using transformers + peft + bitsandbytes (Linux/Win) or MLX-LM (macOS) for QLoRA
- [ ] Three default LoRA roles: `summarizer-personal`, `extractor-personal`, `prefs-personal`
- [ ] Training data builder: pulls from accepted/rejected edits in tm-trace + commitment outcome history
- [ ] Nightly schedule: opt-in only, runs when on AC + idle, capped at 30min wall clock
- [ ] LoRA weights stay in `~/.tracemind/adapters/` — never leave device, no telemetry
- [ ] Lightweight TRL/Unsloth alternative path for low-RAM machines (8GB target)
- [ ] CLI: `tracemind finetune status / start / stop / rollback`
