# Phase 5 — Convergence: from personal memory to intent engine

**Status**: proposal — 2026-05-07
**Author**: Aaditya + Claude (co-founder brainstorm)
**Predecessors**: `PHASE4_DELIGHT.md` (companion UX), `INTENT_SYSTEM.md` (commitment primitive), `BRAIN_ARCHITECTURE.md` (cognitive subsystem map, Sprints A–G)
**Companions**: `PRODUCT_PORTFOLIO.md` (what we ship), `UNIFIED_ARCHITECTURE.md` (shared engine)
**Scope**: quality improvements to TraceMind + engine extensions that enable the Engram and Rosetta product surfaces

---

## 0. Why Phase 5 exists

Phases 3 and 4 gave TraceMind an answer layer and a commitment
primitive. The seven-sprint roadmap (A–G in `BRAIN_ARCHITECTURE.md`)
takes the consumer product from "database with a CLI" to "system of
intents that anticipates your next decision."

Phase 5 answers two questions that Phase 4 left open:

1. **Quality**: How do we close the gap from F1 25.70 (current Tier-0)
   to the 85+ F1 north star? The answer isn't "just wire up Tier-1"
   — it's a systematic quality stack across retrieval, reasoning,
   synthesis, and evaluation.

2. **Platform**: How do we extend the engine so it serves not just
   personal memory (TraceMind) but also agent memory (Engram) and
   code-intent reasoning (Rosetta) — without fragmenting the codebase?

The answer to both is the same: **make the engine deeper** (bitemporal,
cross-modal, belief-native) rather than wider (more features on a
shallow foundation). Depth compounds across products; width doesn't.

---

## 1. Quality north star

### 1.1 Where we are

| Metric | Current | Target | Gap |
|---|---|---|---|
| LoCoMo F1 (mini, Tier-0) | 25.70 | — | — |
| LoCoMo F1 (mini, Tier-1) | not measured | ≥ 60 | Sprint A (prerequisite) |
| LoCoMo F1 (full, Tier-1) | not measured | ≥ 70 | Phase 5A exit (Month 3) |
| LoCoMo F1 (full, all improvements) | not measured | ≥ 85 (north star) | Phase 5C exit (Month 9) |
| Commitment outcome accuracy | not measured | ≥ 60% held-out | Sprint G |
| Cross-modal retrieval MAP | not measured | ≥ 0.5 | Phase 5 |
| Bitemporal query correctness | not measured | 100% (formal) | Phase 5 |

### 1.2 The quality stack

Quality at F1 85+ requires improvements at every layer, not just a
better model:

```
Layer 5: Synthesis quality ─── Tier-1 LLM (Sprint A) + narration (Sprint B)
Layer 4: Reasoning quality ─── cross-modal chains + belief-aware ranking
Layer 3: Retrieval quality ─── arm 5 (cross-modal) + bitemporal filter
Layer 2: Encoding quality ──── multi-modal encoders + shared projection
Layer 1: Ingestion quality ─── cross-modal ingest + temporal metadata
Layer 0: Storage quality ───── bitemporal graph + TMS consistency
```

Each layer addresses a different failure mode:

| Layer | Failure mode it fixes | Expected F1 lift |
|---|---|---|
| 5 (synthesis) | Wrong answer despite correct retrieval | +20–30 (Tier-0 → Tier-1) |
| 4 (reasoning) | Missing connections between related facts | +5–10 |
| 3 (retrieval) | Wrong facts retrieved; temporal mismatch | +5–10 |
| 2 (encoding) | Missed semantic matches across modalities | +3–5 |
| 1 (ingestion) | Important signals dropped at ingest time | +3–5 |
| 0 (storage) | Stale/contradicted facts pollute retrieval | +2–3 |

Cumulative: 25.70 + 20 + 5 + 5 + 3 + 3 + 2 = ~63 (conservative) to
25.70 + 30 + 10 + 10 + 5 + 5 + 3 = ~89 (optimistic).

The quality target is **staged**: F1 ≥ 70 at Phase 5A exit (Month 3),
then F1 ≥ 85 at Phase 5C exit (Month 9). The 70 target is achievable
with conservative estimates (Tier-1 lift + basic retrieval
improvements). The 85 target requires the optimistic end of cross-modal
and belief-aware improvements to land — if we hit 75-80 instead, that's
still competitive and we iterate from there. 85 is a north star, not a
gate.

### 1.3 Evaluation infrastructure

Before we can improve quality, we need to measure it properly:

1. **Full LoCoMo run** — move from the 20-question mini-set to the
   ~7,000-question official benchmark. The mini-set F1 (25.70) is
   directional only and not directly comparable to full-set scores
   reported by competitors (see `LOCOMO_RESULTS.md` for caveats).
   CI regression gate on both mini (fast, every PR) and full (nightly).
2. **Cross-modal eval** — new benchmark: given a multi-modal evidence
   set (screenshots + code + chat), can the system reconstruct the
   correct reasoning chain? Hand-labeled eval set of 50 scenarios.
3. **Bitemporal eval** — formal correctness tests: assert a timeline
   of facts + retractions, verify that every temporal query returns
   exactly the right facts. This is testable to 100%.
4. **Intent preservation eval** — for code-intent: given a known
   refactoring pair (before/after), does `tm-semcode` correctly
   identify that intent was preserved? 100 labeled pairs.
5. **Belief consistency eval** — for TMS: given a set of beliefs with
   justifications, does the TMS correctly identify all contradictions
   and propagate all retractions? Formal verification against the JTMS
   spec.

---

## 2. Phase 5 sprints

Phase 5 adds **8 new sprints** (H through O), continuing the letter
sequence from Sprints A–G in `BRAIN_ARCHITECTURE.md`. Organized as three
sub-phases corresponding to the three products in the portfolio. Each
sub-phase delivers engine improvements that benefit all products, plus a
product-specific surface.

### 2.1 Phase 5A — Cross-modal TraceMind (Month 1–3)

**Theme**: TraceMind reasons across modalities and through time. The
daily brief cites screenshots alongside text. The system knows when a
fact was retracted.

#### Sprint H — Cross-modal foundation

| ID | Title | Crate |
|---|---|---|
| TM-5A-001 | `tm-modal` crate: `ModalNode`, `ModalEncoder` trait, `CrossModalEdge` types | tm-modal (NEW) |
| TM-5A-002 | SigLIP-small ONNX image encoder + shared projection layer | tm-modal |
| TM-5A-003 | Code encoder: tree-sitter AST → BGE embedding | tm-modal |
| TM-5A-004 | Co-occurrence edge generator (temporal proximity) | tm-modal |
| TM-5A-005 | `tm-ingest` extended: `ModalIngestPipeline` for image + code | tm-ingest |
| TM-5A-006 | `tm-graph` extended: `ModalNode` storage, `CrossModalEdge` relations | tm-graph |
| TM-5A-007 | `tm-vector` extended: per-modality + shared-projection indices | tm-vector |
| TM-5A-008 | Cross-modal eval harness (50 hand-labeled scenarios) | tm-bench |

**Exit criteria**: ingest a mixed set (text + screenshots + code files),
query in natural language, get results that correctly span modalities
with grounded citations. Cross-modal MAP ≥ 0.3 on eval set.

#### Sprint I — Bitemporal + TMS + intent arc foundation

| ID | Title | Crate |
|---|---|---|
| TM-5A-009 | `tm-temporal` crate: bitemporal schema extensions, `TemporalQuery` API | tm-temporal (NEW) |
| TM-5A-010 | `tm-graph` migration: add `valid_from`, `valid_to`, `retracted_at` columns + `needs`, `sentiments`, `actions` tables | tm-graph |
| TM-5A-011 | `tm-tms` crate: JTMS engine, `Justification`, `BeliefStatus`, `Contradiction` | tm-tms (NEW) |
| TM-5A-012 | `tm-intent` extended: `Need`, `Sentiment`, `Action` types + `Belief` trait + bitemporal fields on all intent arc types | tm-intent |
| TM-5A-012a | `NeedDetector` in `tm-capture`: mine need-shaped phrases ("I need", "we have to", "the problem is") from existing capture streams | tm-capture |
| TM-5A-012b | `SentimentScorer`: heuristic valence scoring from word choice (positive/negative/neutral × intensity); Tier-1 LLM-assisted when available | tm-intent |
| TM-5A-012c | `ActionMatcher`: match detected actions (git commits, sent messages) to open commitments by embedding similarity + temporal proximity | tm-intent |
| TM-5A-012d | Graph edges for intent arc: `Need ──spawns──► Commitment`, `Commitment ──acted_on_by──► Action`, `* ──felt_as──► Sentiment` | tm-graph |
| TM-5A-013 | `tm-reason` extended: `CrossModalChainBuilder`, `BeliefChainBuilder`, `IntentArcChainBuilder` (traverse full need→outcome chain) | tm-reason |
| TM-5A-014 | `tm-retrieval` extended: arm 5 (cross-modal), bitemporal range filter, belief-aware ranking | tm-retrieval |
| TM-5A-015 | `tm-reflect` extended: TMS consistency check in nightly pass, contradiction surfacing in brief, need recurrence detection | tm-reflect |
| TM-5A-016 | Bitemporal eval suite (formal correctness tests) | tm-bench |
| TM-5A-017 | TMS eval suite (JTMS spec compliance) | tm-bench |

**Exit criteria**: time-travel queries work ("what did I believe last
Tuesday?"). Contradictions detected and surfaced in daily brief. All
bitemporal eval tests pass. TMS spec compliance 100%. Need and sentiment
extraction running on capture streams. Actions auto-linked to
commitments.

#### Sprint J — Integrated quality push

| ID | Title | Crate |
|---|---|---|
| TM-5A-018 | Full LoCoMo benchmark (7k questions) with Tier-1 | tm-bench-locomo |
| TM-5A-019 | Cross-modal reasoning in daily brief (screenshots cited alongside text) | tm-reflect |
| TM-5A-020 | Time-machine queries in CLI + MCP ("what was I thinking in March?") | tm-cli, tm-mcp |
| TM-5A-021 | Belief-aware retrieval: boost `In`, demote `Contradicted`, hide `Out` | tm-retrieval |
| TM-5A-022 | Cross-modal co-occurrence promotion in nightly consolidation | tm-reflect |
| TM-5A-023 | MCP tools: `memory_world_at`, `memory_contradictions`, `memory_cross_modal`, `memory_need`, `memory_sentiment`, `memory_action`, `memory_arc` | tm-mcp |
| TM-5A-025 | Daily brief extended: surfaces recurring needs, sentiment trends, commitment-action gaps ("you said X but haven't acted") | tm-reflect |
| TM-5A-026 | Intent arc visualization in Tauri: need → sentiment → commitment → action → outcome chain view | tm-tauri |
| TM-5A-024 | Regression gate: LoCoMo F1 ≥ 70 on full set, cross-modal MAP ≥ 0.5 | CI |

**Exit criteria**: LoCoMo F1 ≥ 70. Cross-modal MAP ≥ 0.5. Time-travel
queries work end-to-end. Contradictions surfaced correctly. The daily
brief is noticeably richer — it references screenshots and notes when
facts changed.

**Felt**: *"It showed me the screenshot AND the Slack message AND knew
the fact changed last week. It told me I've been frustrated about CI
for three weeks and still haven't acted on my migration commitment.
This thing actually understands me."*

---

### 2.2 Phase 5B — Engram extraction (Month 4–6)

**Theme**: Extract the engine as an agent memory SDK. Ship Engram as an
MCP server + Rust crate. Agent developers can use bitemporal beliefs
with truth maintenance out of the box.

#### Sprint K — Engram SDK

| ID | Title | Crate |
|---|---|---|
| TM-5B-001 | `Belief` trait in `tm-intent` — all intent arc types implement it; generalized for agents | tm-intent |
| TM-5B-002 | `tm-engram` crate: wraps engine as belief-native Rust SDK with full intent arc support (goals, confidence, actions, observations) | tm-engram (NEW) |
| TM-5B-003 | Engram API: `assert()`, `retract()`, `world_at()`, `contradictions()`, `history_of()`, `set_goal()`, `record_action()`, `observe()` | tm-engram |
| TM-5B-004 | MCP tools: `memory_believe`, `memory_retract` | tm-mcp |
| TM-5B-005 | Engram MCP server binary (standalone, no TraceMind dependencies) | tm-engram |
| TM-5B-006 | Python wrapper via PyO3 (publish to PyPI as `engram`) | tm-engram |
| TM-5B-007 | TypeScript wrapper via napi-rs (publish to npm as `@tracemind/engram`) | tm-engram |
| TM-5B-008 | Integration tests: Claude Code agent using Engram MCP for persistent memory | tm-bench |
| TM-5B-009 | Publish Rust crate to crates.io | — |

**Exit criteria**: an agent developer can `pip install engram` or
`npm install @tracemind/engram`, spin up an MCP server, and have
bitemporal belief management with truth maintenance working in under
5 minutes. Integration test passes with a real Claude Code session.

#### Sprint L — Engram quality + docs

| ID | Title | Crate |
|---|---|---|
| TM-5B-010 | Engram benchmark: belief assertion throughput, retraction propagation latency | tm-bench |
| TM-5B-011 | Engram documentation: getting started, API reference, architecture guide | docs/ |
| TM-5B-012 | Example agents: a research assistant, a code review agent, a customer support agent — all using Engram for memory | examples/ |
| TM-5B-013 | Engram landing page / README with positioning | — |
| TM-5B-014 | Metered API scaffolding (usage tracking, rate limiting) | tm-engram |

**Exit criteria**: docs + examples published. Three example agents
working. Benchmark shows <1ms assertion, <10ms retraction propagation
for 10k beliefs.

**Felt**: *"Finally, agent memory that isn't just vector search with a
timestamp."*

---

### 2.3 Phase 5C — Rosetta foundation (Month 7–9)

**Theme**: Ship the first code-intent capabilities. Start with intent
extraction and semantic diff — the hardest technical problems. Migration
comes later.

#### Sprint M — Code-intent extraction

| ID | Title | Crate |
|---|---|---|
| TM-5C-001 | `tm-semcode` crate: `CodeIntent`, `CodeTarget`, `IntentEvidence` types | tm-semcode (NEW) |
| TM-5C-002 | Tree-sitter parsing for Rust, Python, TypeScript, Go, Java | tm-semcode |
| TM-5C-003 | Intent extraction pipeline: code + tests + docs + commits → `CodeIntent` | tm-semcode |
| TM-5C-004 | `tm-graph` integration: `CodeIntent` as entities, intent-relationship edges | tm-graph |
| TM-5C-005 | Cross-modal intent evidence: `tm-modal` connects code spans to PR discussions | tm-modal |
| TM-5C-006 | Intent preservation eval harness (100 labeled refactoring pairs) | tm-bench |

**Exit criteria**: point `tm-semcode` at a real Rust crate (e.g.,
`tm-types`), get back a set of `CodeIntent` records that correctly
describe what each function/type is for. Eval set accuracy ≥ 70%.

#### Sprint N — Semantic diff + drift detection

| ID | Title | Crate |
|---|---|---|
| TM-5C-007 | `SemanticDiff` engine: compare two code snapshots at the intent level | tm-semcode |
| TM-5C-008 | `SemanticChangeType` classification (Cosmetic / Refactor / IntentShift / ...) | tm-semcode |
| TM-5C-009 | Intent drift detection: bitemporal tracking of `CodeIntent` over git history | tm-semcode + tm-temporal |
| TM-5C-010 | `tm-rosetta` crate: CLI wrapping `tm-semcode` for developer use | tm-rosetta (NEW) |
| TM-5C-011 | MCP tools: `code_intent`, `code_semantic_diff` | tm-mcp |
| TM-5C-012 | VS Code extension stub (semantic diff view) | separate repo |

**Exit criteria**: `rosetta diff HEAD~5..HEAD` shows semantic changes
for the last 5 commits. Intent drift detection flags functions whose
purpose has silently expanded. Semantic change classification accuracy
≥ 75% on eval set.

#### Sprint O — Intent-preserving transformations (stretch)

| ID | Title | Crate |
|---|---|---|
| TM-5C-013 | Intent-preserving refactoring: given a CodeIntent + target pattern, generate refactored code | tm-semcode |
| TM-5C-014 | Cross-language migration prototype: Python → Rust for a single module | tm-semcode |
| TM-5C-015 | Property-based test generation: automatically test that transformation preserved intent | tm-semcode |
| TM-5C-016 | GitHub Action: flag PRs where intent drifted | separate repo |

**Sprint O is a stretch goal.** It is not committed scope. The exit
criteria below are aspirational — if Sprint O doesn't land, Phase 5C
is still a success as long as Sprints M and N deliver.

**Exit criteria (stretch)**: demonstrate one successful Python → Rust
migration of a small module (~200 LOC) where the generated Rust code is
idiomatic AND the generated property tests pass. This is a demonstration
of capability, not a production-ready migration tool.

**Felt**: *"It told me the commit changed authorization from role-based
to attribute-based. It didn't just show me a diff — it showed me what
changed in meaning."*

---

## 3. Quality improvements detail

### 3.1 Retrieval quality (Sprints H–J)

**Problem**: Current retrieval is text-only, temporal-unaware, and
treats all facts as equally valid regardless of whether they've been
retracted.

**Improvements**:

1. **Cross-modal arm (arm 5)**: when the query touches multiple
   modalities ("show me the bug screenshot from last week"), arm 5
   searches across modality-specific indices and fuses with
   modality-weighted Reciprocal Rank Aggregation.

2. **Bitemporal range filter**: when the query has temporal intent
   ("what did I think in March?"), the retrieval pipeline applies
   `valid_at` and `known_as_of` filters before ranking. This prevents
   anachronistic results.

3. **Belief-aware ranking**: results with TMS status `In` get a boost;
   `Contradicted` results get demoted (but shown with a contradiction
   indicator); `Out` results are hidden unless the user explicitly
   requests retracted facts.

4. **Cross-modal co-occurrence boost**: when a text result has a
   `CoOccurrence` edge to a screenshot taken within 30 seconds, the
   screenshot is promoted as supporting evidence.

### 3.2 Reasoning quality (Sprints H–J)

**Problem**: Current reasoning chains are text-only and don't account
for belief status.

**Improvements**:

1. **Cross-modal chains**: a reasoning chain can now traverse text →
   screenshot → code → chat. Each hop carries its modality and
   evidence, making the chain auditable across modalities.

2. **Belief-justified chains**: every step in a reasoning chain
   carries its TMS justification status. If a supporting belief is
   retracted, the chain's conclusion is flagged. "This conclusion
   held on March 15 but is no longer supported because belief X was
   retracted on April 2."

3. **Intent-arc chains**: reasoning chains can now traverse the full
   intent arc. "Why did we switch CI?" → need (faster deployments) →
   sentiment (frustrated with old system) → commitment (migrate by
   June) → action (PR #47) → outcome (60% faster). Each node is a
   different intent-arc type, all grounded with evidence.

4. **Intent-aware code chains**: for code-related queries, chains can
   traverse intent relationships. "Why does this function exist?" →
   commit C created it → PR P motivated it → issue I required it.

### 3.3 Synthesis quality (Sprint A + ongoing)

**Problem**: Tier-0 extractive synthesis is the primary bottleneck
(F1 25.70). Tier-1 is expected to close most of the gap.

**Improvements**:

1. **Tier-1 default** (Sprint A): Wire `tm-answer::LocalLlmBackend`
   to `llama-cpp-2` with Qwen 2.5 1.5B Q4. Expected lift: +20–30 F1.

2. **Grounded synthesis**: every sentence in the generated answer must
   cite at least one source. The synthesis prompt enforces this; a
   post-generation check strips uncited claims.

3. **Cross-modal citations**: answers can cite screenshots and code
   spans, not just text. "The bug was visible in [screenshot from
   Tue 14:31] and caused by [webhook.rs:47, commit a3f9c01]."

4. **Contradiction awareness**: when the synthesis encounters
   contradicting sources, it notes the contradiction rather than
   picking one. "Source A says X; source B says Y (source B is more
   recent)."

### 3.4 Encoding quality (Sprint H)

**Problem**: Text-only embeddings miss semantic matches across
modalities.

**Improvements**:

1. **Multi-modal encoders**: SigLIP for images, Whisper features for
   audio, tree-sitter + BGE for code. All project to shared 384-dim.

2. **Fine-tuned projection layers**: the per-modality projection is
   initialized with pretrained weights and fine-tuned on the user's
   own cross-modal co-occurrences (a screenshot taken alongside a
   commit message is a positive training pair).

3. **Modality-aware similarity**: cosine similarity in the shared
   space is modality-normalized (image-text similarity has a different
   threshold than text-text).

### 3.5 Ingestion quality (Sprints H, I)

**Problem**: Important signals are dropped at ingest time because the
pipeline only handles text.

**Improvements**:

1. **`ModalIngestPipeline`**: extends the existing text pipeline with
   image, audio, code, and structured data paths. Each path runs
   governance gate → modality-specific encoding → graph storage →
   co-occurrence edge generation.

2. **Temporal metadata at ingest**: every ingested signal carries
   `valid_from` (when the fact became true) and `txn_at` (now). For
   most captures, `valid_from` = `txn_at`. For imports (Obsidian vault,
   git history), `valid_from` is extracted from the source's timestamps.

3. **Salience model upgrade**: the salience model (Loop 3) gains
   cross-modal features. A screenshot taken alongside a high-stakes
   commitment is more salient than a routine clipboard copy.

### 3.6 Storage quality (Sprint I)

**Problem**: Stale and contradicted facts pollute retrieval results.

**Improvements**:

1. **Bitemporal storage**: facts carry valid-time ranges. Retracted
   facts are preserved (immutable audit trail) but marked with
   `retracted_at` and `retraction_reason`.

2. **TMS consistency**: the nightly consolidation pass runs the TMS
   over all beliefs. Contradictions are surfaced in the daily brief.
   Unsupported beliefs (all justifications broken) are demoted in
   retrieval ranking.

3. **Temporal garbage collection**: facts retracted more than N days
   ago (configurable, default 365) are archived to a cold store
   (compressed SQLite backup). They remain queryable via explicit
   time-travel queries but don't participate in regular retrieval.

---

## 4. Engine improvements that benefit all products

Every improvement in Phase 5 is designed to compound across the
portfolio. Here's the map:

| Improvement | TraceMind benefit | Engram benefit | Rosetta benefit |
|---|---|---|---|
| **Cross-modal encoders** | Richer daily brief (cites screenshots) | Agents ground beliefs in images/code | Intent extracted from code + docs + PRs |
| **Bitemporal storage** | "What did I believe in March?" | "What did agent know at time T?" | "When did this function's intent change?" |
| **TMS** | Contradictions in daily brief | Auto-retraction propagation for agents | Intent-consistency checking across refactors |
| **Arm 5 (cross-modal)** | Better retrieval across modalities | Agents retrieve across tool outputs | Code + test + doc unified search |
| **Belief-aware ranking** | Fresh facts ranked higher | Agents don't act on retracted beliefs | Current intent ranked over historical |
| **Cross-modal chains** | "Find the screenshot, the commit, the thread" | Multi-tool reasoning chains | "Why does this function exist?" chains |
| **Intent arc (need→sentiment→commitment→action→outcome)** | "Why did I decide this? How did I feel?" Full decision lifecycle | Agent goals→beliefs→actions→observations as one chain | Requirements→design→implementation traceability |
| **Need/sentiment extraction** | Surfaces recurring needs, emotional context in brief | Agent goal detection and confidence tracking | Tech debt frustration → migration priority signal |
| **Full LoCoMo** | Regression gate for consumer quality | Regression gate for agent quality | Regression gate for code-intent quality |

---

## 5. Risk assessment

### 5.1 Risks carried forward from Phase 4

1. **Cold-start trap** — mitigated by L1 prefetch from day 1 + sample
   data onboarding.
2. **Wrong-prediction recovery** — mitigated by calibration panel +
   auto-quiet.
3. **Anthropomorphic creep** — mitigated by statistical tone guide.

### 5.2 New risks in Phase 5

4. **Cross-modal complexity explosion.** Five modalities × bitemporal ×
   TMS = large surface area.
   *Mitigation*: Phase 5A starts with two modalities (text + image) and
   adds others incrementally. The `ModalEncoder` trait makes adding
   modalities a bounded effort (~200 LOC adapter each).

5. **Bitemporal query performance.** Adding temporal columns to every
   entity/relation could slow queries.
   *Mitigation*: temporal columns are nullable with defaults. Queries
   without temporal filters ignore them entirely (no performance
   impact on existing code paths). Temporal queries use SQLite
   partial indexes on `valid_from`/`valid_to`.

6. **TMS false positives.** The contradiction detector might flag
   things that aren't actually contradictions.
   *Mitigation*: high threshold for contradiction detection
   (cosine < -0.8 for semantic negation, or explicit schema
   constraint violation). Under-detection is the safe failure mode.

7. **Code-intent extraction quality.** LLM-free intent extraction
   from code is hard.
   *Mitigation*: Phase 5C starts with Tier-1 LLM-assisted extraction
   and falls back to heuristic (function name + doc comment + test
   name) when LLM is unavailable. The eval harness (100 labeled pairs)
   gates quality before shipping.

8. **Portfolio distraction.** Working on three products could fragment
   focus.
   *Mitigation*: Staged unfold. Phase 5A is 100% TraceMind. Engram
   extraction (5B) only starts after 5A proves the engine. Rosetta
   (5C) only starts after 5B ships. One product surface at a time.

9. **Sentiment inference accuracy.** Heuristic valence scoring from
   word choice will be noisy. "I'm killing it" is positive but contains
   a negative word.
   *Mitigation*: start with simple signal (explicitly positive/negative
   phrases) and improve with Tier-1 LLM-assisted scoring when
   available. Sentiment is always shown with its evidence text so the
   user can correct. Wrong sentiment is low-cost (it's context, not a
   recommendation). Under-extraction is the safe default — only surface
   sentiment when confidence is high.

10. **Need/action over-extraction.** Mining "I need" from every
    clipboard capture could generate noise.
    *Mitigation*: same pattern as CommitmentMiner — candidates surfaced
    in daily brief for one-tap confirm/dismiss. Dismissed candidates
    train the threshold. N≥3 repetitions of the same need before it's
    promoted to a stable Need record.

11. **Model download size.** Adding SigLIP (~100MB) on top of BGE
   (~30MB) + ColBERT (~35MB) + Tier-1 LLM (~900MB) grows the total
   to ~1.1GB.
   *Mitigation*: model downloads are incremental and feature-gated.
   Users who don't use image capture don't download SigLIP. The
   first-run experience downloads only the minimum (BGE + Tier-1).

---

## 6. Success metrics

### Phase 5A (Month 3 exit)

| Metric | Target |
|---|---|
| LoCoMo F1 (full, Tier-1) | ≥ 70 |
| Cross-modal retrieval MAP | ≥ 0.5 |
| Bitemporal query correctness | 100% (formal test suite) |
| TMS spec compliance | 100% (formal test suite) |
| Need extraction precision | ≥ 70% (confirmed by user in daily brief) |
| Sentiment scoring accuracy | ≥ 65% valence correctness on labeled sample |
| Action-to-commitment auto-link rate | ≥ 50% of actions correctly linked |
| Intent arc completeness | ≥ 30% of commitments have at least one linked need OR action |
| New crates | tm-modal, tm-temporal, tm-tms |
| New bandit arm | arm 5 (cross-modal) |
| New intent arc types | Need, Sentiment, Action in tm-intent |

### Phase 5B (Month 6 exit)

| Metric | Target |
|---|---|
| Engram assertion latency | < 1ms |
| Engram retraction propagation (10k beliefs) | < 10ms |
| Engram published on | crates.io, PyPI, npm |
| Integration test with Claude Code | passing |
| Example agents | 3 working |

### Phase 5C (Month 9 exit)

| Metric | Target |
|---|---|
| Intent extraction accuracy | ≥ 70% on eval set |
| Semantic change classification | ≥ 75% on eval set |
| Languages supported | Rust, Python, TypeScript, Go, Java |
| Cross-language migration demo | 1 working (Python → Rust) |
| LoCoMo F1 (full, all improvements) | ≥ 85 (north star; ≥ 75 is acceptable) |

---

## 7. Relationship to existing roadmap

Phase 5 **follows** the seven-sprint roadmap (A–G) from
`BRAIN_ARCHITECTURE.md`. It does not replace it — Sprints A–G deliver
the core TraceMind consumer product. Phase 5 extends the engine with
cross-modal, bitemporal, and code-intent capabilities.

```
Sprints A–G (BRAIN_ARCHITECTURE.md)
├── A: Tier-1 LLM, real answers
├── B: Commitment primitive, L1 prefetch, voice
├── C: Outcomes, patterns, multimodal capture
├── D: Reward circuit (preference + counterfactual)
├── E: Visible surface, L3 gated
├── F: Mobile + photos + sync
└── G: World model v2 + scale

Phase 5 (this doc) — follows A–G, extends engine
├── 5A: Cross-modal + bitemporal + TMS into TraceMind (Sprints H–J)
├── 5B: Engram SDK extraction (Sprints K–L)
└── 5C: Rosetta code-intent foundation (Sprints M–O)
```

**Dependency**: Phase 5A requires Sprint A (Tier-1 LLM) and Sprint B
(tm-intent, tm-reflect) to be complete. Sprint A's LoCoMo F1 ≥ 60
(mini, Tier-1) is a hard prerequisite — if Tier-1 doesn't deliver that
baseline, the Phase 5 quality targets are not credible and we need to
diagnose before proceeding. Phase 5A can run in parallel with Sprints
C–G for the non-overlapping work.

**Integration points**: Sprint C's "screenshot capture + VLM caption"
becomes Phase 5A's `ModalIngestPipeline` for images. Sprint C's
"contradiction detection" becomes Phase 5A's TMS. Sprint G's "world
model v2" benefits from the richer cross-modal + bitemporal features
in its training data.

---

## 8. Decisions to confirm

1. **Phase 5A starts after Sprint A+B complete.** The cross-modal and
   bitemporal work requires Tier-1 LLM and tm-intent to be stable.
2. **Engram is extracted, not forked.** `tm-engram` wraps the same
   engine crates TraceMind uses. No code duplication.
3. **Rosetta starts with extraction + diff, not migration.** Intent
   extraction and semantic diff are the foundation. Cross-language
   migration is Sprint O (stretch goal) — it needs the extraction to be
   solid first.
4. **Two modalities first (text + image), then expand.** Audio and
   structured data adapters come after text + image are proven.
5. **LoCoMo F1 ≥ 85 is the quality north star for Phase 5.** This is
   the number that makes TraceMind competitive with cloud-AI-memory on
   answer quality while winning on privacy, breadth, and learning.
6. **Model downloads are incremental and feature-gated.** No user is
   forced to download models for modalities they don't use.
7. **Intent is the full arc: Need → Sentiment → Commitment → Action →
   Outcome.** The existing `Commitment` primitive is extended, not
   replaced. Need and Sentiment extraction starts heuristic (regex +
   word choice), upgrades to Tier-1 LLM-assisted when available.
   Action matching is embedding-based. All new types are optional —
   a Commitment without a linked Need still works exactly as before.
