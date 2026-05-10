# TraceMind — Design, Decisions & Concepts

**Consolidated**: 2026-05-09. Replaces `BRAIN_ARCHITECTURE.md`, `INTENT_SYSTEM.md`, `PHASE4_DELIGHT.md`, `PRODUCT_PORTFOLIO.md`, `UNIFIED_ARCHITECTURE.md`, `PHASE5_CONVERGENCE.md`, `LOCOMO_RESULTS.md`.

---

## 1. What TraceMind is

TraceMind is a **system of intents**: it captures what you commit to, the context you had when you decided, and what actually played out — then anticipates the decisions you're about to make using only your own track record. Nothing leaves your machine. Ever.

The unit is the **intent arc**: `Need → Sentiment → Commitment → Action → Outcome`. Intent is the invariant that persists through changes in form (text/voice/code), modality (screenshot/commit/message), and validity over time (retraction, supersession).

### 1.1 Three-product portfolio, one engine

| Product | Question it answers | Phase |
|---|---|---|
| **TraceMind** | "What did I intend, what happened, what should I expect next?" | 1 (now) |
| **Engram** | "What did the agent believe, when did it change, and why?" | 2 (Month 4+) |
| **Rosetta** | "What does this code intend, and does the transformation preserve it?" | 3 (Month 7+) |

All three share the same Rust engine. Every engine improvement pays dividends across all three. TraceMind is the proving ground — stress-tests the engine with real users before Engram or Rosetta depend on it.

### 1.2 Positioning

- **Externally**: "a system of intents" — no "brain" or "second brain" branding
- **Internally**: brain-shaped cognitive architecture (working/episodic/semantic memory, default-mode reflection, reward circuit)
- **Wedge persona**: solo founders/indie operators (Aaditya's own profile, tight dogfood)
- **Competitive moat**: intent-aware substrate on your hardware vs. cloud-attached saved-prompts (ChatGPT Memory, Mem0, Rewind)

---

## 2. Intent arc data model

```
Need → Sentiment → Commitment → Action → Outcome
WHY    HOW I FEEL   WHAT I       WHAT I   WHAT
I care ABOUT IT     PLAN TO DO   DID      HAPPENED
```

### 2.1 Commitment (core primitive, shipped)

```rust
Commitment { id, kind: Intent|Decision|Hypothesis, statement, made_at,
  horizon, context_snapshot_id, options_considered, chosen, confidence,
  expected_outcome, stakes: Low|Medium|High|Reversible,
  state: Open|Acted|Completed|Abandoned|Superseded,
  outcome_id, derived_from, tags, source: Manual|VoiceCapture|ImplicitMined|McpStructured }
```

### 2.2 Outcome (shipped)

```rust
Outcome { id, commitment_id, observed_at, description,
  polarity: Better|AsExpected|Worse|Mixed|NoOutcome,
  surprise, evidence_traces, user_note, source: UserPrompted|ImplicitMatched|McpStructured }
```

### 2.3 Need (shipped)

```rust
Need { id, statement, urgency: 0.0..1.0, recurring: bool,
  first_seen, last_seen, source: Explicit|Mined|Inferred|McpStructured,
  linked_commitments, tags }
```

### 2.4 Sentiment (shipped)

```rust
Sentiment { id, target_id, target_type: Commitment|Need|Entity|Topic,
  valence: -1.0..1.0, intensity: 0.0..1.0,
  source: Heuristic|LlmAssisted|UserProvided|McpStructured,
  captured_at, evidence_text, evidence_trace }
```

### 2.5 Action (shipped)

```rust
Action { id, commitment_id, description, taken_at, evidence,
  modality: Digital|Physical|Communication|Creation,
  source: Detected|UserReported|McpStructured|EmbeddingMatched }
```

### 2.6 Anticipation (shipped)

```rust
Anticipation { id, generated_at, trigger: TriggerContext,
  kind: PrefetchQuery|PatternMatch|Recommendation,
  grounded_in, confidence, user_response: Accepted|Dismissed|Starred|Silenced|Ignored,
  eventual_match, expires_at }
```

### 2.7 Belief trait (shipped)

All intent-arc types implement `Belief { belief_id(), statement(), confidence(), created_at() }` — enables the Engram SDK to treat everything uniformly.

### 2.8 State machine

```
(capture/mine/MCP) → Open → Acted → Completed (outcome attached)
                       ├── → Abandoned (user-initiated or 4× horizon with no signal)
                       └── → Superseded (new Commitment with derived_from)
```

---

## 3. Architecture

### 3.1 Crate map (24 crates)

```
binaries ─────────── tm-cli, tm-mcp, tm-capture, tm-tauri
answer layer ──────── tm-answer (Tier 0/1/2 dispatcher)
retrieval ─────────── tm-retrieval → tm-rerank (ColBERT) → tm-reason (chains, analogy, causal)
                                   → tm-controller (UCB1 + LinUCB, 5 arms)
ingest ────────────── tm-ingest → tm-governance (PII + confidence gate)
reflection ────────── tm-reflect (brief, patterns, outcome matcher, insights)
intent ────────────── tm-intent (Commitment, Need, Sentiment, Action, Outcome, state machine)
world model ───────── tm-world-model (f_outcome logistic regression v0)
storage ───────────── tm-graph (SQLite KG), tm-vector (BGE-small ONNX, 384d)
                      tm-episodic (traces, trajectories, procedures, recent)
new engine crates ─── tm-temporal (bitemporal facts), tm-tms (JTMS truth maintenance)
                      tm-modal (cross-modal reasoning), tm-engram (Engram SDK)
benchmarks ────────── tm-bench, tm-bench-locomo
core types ────────── tm-types (zero I/O)
```

### 3.2 Cognitive subsystem mapping

```
PERCEPTION (tm-capture, tm-ingest, tm-voice*)
  → WORKING MEMORY (tm-types::WorkingMemory* — in-RAM ring of last N turns)
    → EPISODIC (tm-episodic) | SEMANTIC (tm-graph, tm-vector) | PROCEDURAL (ProcedureStore)
      → HIPPOCAMPUS (tm-reason::Consolidator + tm-reflect dreaming*)
        → PREFRONTAL CORTEX (tm-controller, tm-retrieval, tm-reason)
          → DEFAULT MODE NETWORK (tm-reflect: idle pattern detection, daily brief)
            → LANGUAGE/NARRATIVE (tm-narrate*: prose synthesis, citations)
              → REWARD CIRCUIT (tm-controller LinUCB + tm-preference*)
                → INTENT STORE (tm-intent) → WORLD MODEL (f_topic* + f_outcome)
                  → ANTICIPATION GENERATOR (L1 prefetch, L2 patterns, L3 recommendations)
(* = not yet built)
```

### 3.3 Five context graphs (one graph, five views)

1. **Topical** — entities + typed edges (k-hop expansion)
2. **Temporal** — entities by `created_at` + access log
3. **Causal** — `CausalTrace` edges (`A_caused_B`, `A_blocked_B`)
4. **Affective** — edges weighted by thumbs-up/down + dwell-time (not yet wired)
5. **Commitment** — `Commitment ── derived_from ──► Commitment`, `── about ──► Entity`

### 3.4 Data flow

**Ingest**: governance gate → heuristic NER (or GLiNER) → GraphStore.upsert (entities + triples) → VectorStore.embed (BGE-small) → TraceStore.append → RecentStore ring buffer

**Query**: QueryPlanner classifies → LinUcbBandit.select arm → vector search → ColBERT rerank → RRA fusion → graph expansion → MMR diversity → 1-hop related entities → confidence assessment → register_reward

**Answer** (Tier 0/1/2): AnswerRequest → TieredAnswerer selects backend by availability + task kind → AnswerResponse { tier, text, citations, latency_ms }

### 3.5 Bandit arms

| Arm | Name | top_k | hops | episodic | colbert |
|-----|---------|-------|------|----------|---------|
| 0 | narrow | 5 | 0 | no | no |
| 1 | medium | 10 | 1 | no | no |
| 2 | wide | 15 | 2 | no | no |
| 3 | deep | 20 | 2 | yes | no |
| 4 | colbert | 10 | 1 | no | yes |

### 3.6 Tier system

| Tier | Backend | Size | Status | Best for |
|------|----------------------|---------|---------|------|
| 0 | ExtractiveBackend | 0 MB | shipped | always-on fallback; templated synthesis |
| 1 | LocalLlmBackend | ~900 MB | scaffolded | structured extraction, Q&A (Qwen 2.5 1.5B Q4) |
| 2 | AppleFmBackend | 0 MB | scaffolded | open-ended synthesis on macOS 26+ |

### 3.7 Platform tiers

| Device | Model | RAM |
|---|---|---|
| MacBook M-series 16GB+ | Qwen 2.5 1.5B Q4 | ~1.6GB |
| MacBook Intel/8GB | Qwen 0.5B Q4 | ~700MB |
| iOS 26+ | Apple FoundationModels | OS-managed |
| iOS <26 / Android high-end | Qwen 0.5B | ~600MB |
| Android mid/low | Tier-0 + opt-in cloud | ~150MB |

---

## 4. Key design decisions (confirmed)

1. **Internal architecture = brain-shaped; external metaphor = open.** Brain language stays in engineering docs only.
2. **Quality is the constraint, not RAM.** Platform-specific tiers govern footprint per device.
3. **Wedge = solo founder for defaults**, three persona modes (Founder/Researcher/Journaler) at 1.0.
4. **Optional encrypted-cloud Tier**: opt-in only, never default, never required.
5. **System of intents is the wedge.** `Commitment` is the core primitive. Intent is the full arc: Need→Sentiment→Commitment→Action→Outcome.
6. **Three predictive layers** with strict opt-in gating (L1 silent, L2 opt-in after 30 days/50 commitments, L3 explicit opt-in + accuracy gate).
7. **Local-only by default.** No network in the request path. Network only for opt-in model downloads.
8. **SQLite only.** Both `tm-graph` and `tm-vector` use rusqlite (bundled).
9. **Three-product portfolio**: TraceMind, Engram, Rosetta. Staged unfold. Shared engine is the moat.
10. **Engram is extracted (wraps same engine), not forked.** Rosetta starts with extraction + diff (not migration).
11. **World model**: embedding-space only (never generates text about user), per-user local, single-command deletable, calibration-gated.
12. **Multimodal capture is critical on every platform.** Screenshots/voice/photos in early sprints.

---

## 5. Three predictive layers

| Layer | Trigger | Default | Failure mitigation |
|---|---|---|---|
| **L1 Prefetch** | Any interaction start | Always ON (silent) | Wasted work; budget-capped at 50ms |
| **L2 Pattern surfacing** | Context matches a surfaceable cell | OFF for first 30 days/50 commitments | 3 silenced cells → soft pause + ask user |
| **L3 Recommendation** | User explicitly asks; `memory_recommend` MCP | Disabled; requires explicit opt-in AND accuracy gate | Never ships until held-out accuracy > 60% |

L3 always ends with "You have the call." L3 disabled for financial/medical-tagged commitments.

---

## 6. Pattern detector

Runs nightly on `Completed` Commitments in last 12 months:

1. Bucket by cell = (tag intersection × stakes × source_app cluster × time-of-day band × topic centroid bucket, k=16)
2. Per-cell stats: n, polarity distribution, lift = p_worse(cell) - p_worse(global), Wilson-lower-bound support
3. **Surface only if**: n≥6, |lift|≥0.25, support consistent with lift, user hasn't silenced this cell in 90 days, no equivalent pattern surfaced/dismissed in 30 days
4. Fixed template rendering (no model generation — hallucination risk)

---

## 7. World model

Two-headed, embedding-space only:

- **f_outcome v0** (shipped): multinomial logistic regression on metadata features (stakes, time-band, horizon, kind, tags). Persists to `~/.tracemind/world_model.json`. Powers daily brief preflight.
- **f_topic** (not built): 2-layer MLP, 384→512→384, contrastive InfoNCE vs. random negatives. Powers L1 silent prefetch. ~5MB.
- **f_outcome v2** (not built): 4-layer transformer (~3M params), d=256, 4-class polarity + confidence scalar. Powers L2/L3. ~25MB.

Training: entirely per-user. Cold-start: warm init from small public-domain decision corpus. Evaluation: held-out last 10% of resolved Commitments. L2 gate: Brier ≤ 0.30. L3 gate: 4-class accuracy ≥ 60%.

---

## 8. Four learning loops

| Loop | Timescale | Algorithm | Where |
|---|---|---|---|
| 1 — Bandit | Seconds | LinUCB over context vector | tm-controller |
| 2 — Preference | Hours–days | Triplet contrastive loss, 64-dim pref embedding | tm-preference (not built) |
| 3 — Salience | Days–weeks | Logistic regression on capture features | tm-reflect (not built) |
| 4 — Counterfactual | Weekly+ | Doubly-robust OPE over trajectory store | tm-controller::OffPolicyEvaluator (not built) |

---

## 9. New engine capabilities

### 9.1 Bitemporal data (`tm-temporal`, crate exists)

Two time axes on every fact:
- **Valid time** (valid_from, valid_to): when fact was true in reality
- **Transaction time** (txn_at): when system learned it

Enables: "what was true at T?", "what did system know at T?", time-machine queries. Schema extensions needed in `tm-graph` (bitemporal columns on entities/relations, `belief_revisions` table). **Not yet wired into tm-graph.**

### 9.2 Truth maintenance (`tm-tms`, crate exists)

JTMS with BFS propagation. Beliefs are `In`, `Out`, or `Contradicted`. Justifications track evidential, deductive, inductive, and default support. Contradiction detection via: explicit negation (cosine < -0.8), schema constraints, temporal overlap. **Not yet wired into ingest/retrieval.**

### 9.3 Cross-modal reasoning (`tm-modal`, crate exists)

`ModalNode` + `ModalEncoder` trait + `CrossModalEdge` types (CoOccurrence, Contains, References, SemanticLink, Causal). Encoder stubs for text (BGE-small), image (SigLIP-small), audio (Whisper-tiny), code (tree-sitter + BGE). Per-modality linear projection to shared 384-dim space. **Encoders return ModelNotLoaded; not yet wired into retrieval pipeline.**

### 9.4 Engram SDK (`tm-engram`, crate exists)

Wraps IntentStore + TmsEngine + TemporalStore. API: `assert_belief`, `retract`, `world_at`, `contradictions`, `history_of`, `set_goal`, `record_action`, `observe`. **Scaffolded only; no Python/TypeScript wrappers.**

### 9.5 Context segmentation (NEW — Sprint C-0, top priority as of 2026-05-10)

**Wedge-critical insight from 2026-05-10 investor review:** The local-only thesis only works if TraceMind respects context boundaries. A user's laptop is the *most* context-blurred surface in their digital life — multiple jobs, multiple ventures, personal life all on one machine. A "memory OS" that conflates unrelated contexts is worse than separate cloud accounts. The sharper wedge sentence: **"the on-device memory that knows when *not* to connect dots."**

Concretely, this turns three currently-default behaviors into context-discipline failures:
1. Related-entity 1-hop expansion fires across *all* contexts → bridges unrelated work streams.
2. Vector retrieval scores semantic similarity context-agnostically → pulls Rondo paragraphs into TraceMind queries.
3. The bandit rewards "relevant retrieval" with no notion of context-correctness.

**Primitives:**
- `Context { id, name, tags, created_at }` — a named namespace (e.g. "Rondo", "TraceMind", "Personal"). Stored in a `contexts` table.
- `context_id` column on `captured_signals` (raw capture context) and `kg_relations` (asserted-in-context). Entities are context-free (a person named Carol can appear in multiple contexts); triples about Carol *belong to* a context.
- Active context state in `~/.tracemind/active_context.json` — `null` means all-contexts mode (legacy behavior).
- `negative_signals` table — per-result penalty signals (`not_related`, `wrong_context`) that the rerank and bandit consume.

**Default behavior changes:**
- Ingest tags every triple + signal with the active `context_id` (defaulting to a "general" context if none active).
- Retrieval filters `WHERE context_id = ?` by default; `--cross-context` flag opts back into the wide net.
- Related-entity 1-hop expansion only traverses within-context edges by default; cross-context edges require accumulated positive signal *or* explicit user opt-in.
- Bandit reward decomposes: `final_reward = relevance_reward - cross_context_penalty(query, results)`.

**Sprint C-0 deliverables (this week, replaces partial Sprint C-1 priority):**
1. Schema migration: `contexts` table, `context_id` columns, `negative_signals` table.
2. `tracemind context create|list|use|current` CLI.
3. Ingest writes active `context_id` on every triple + signal.
4. Retrieval honors active context (default scoped; `--cross-context` overrides).
5. `tracemind feedback --not-related <query_id> <result_id>` writes a negative signal.
6. Brief surface shows the active context + per-row context tags.
7. Demo replaces "cross-document recall" shot with "scoped recall: same query, two contexts, two answers".

**Why before C-1/C-2:** without `context_id`, the bitemporal + TMS work amplifies bad bridging (a Tier-1 LLM hallucinating across contexts is worse than no Tier-1 LLM). Context discipline is the substrate that makes everything else trustable.

---

## 10. Three capture paths for commitments

1. **Implicit phrase mining** (80% target): CommitmentMiner watches capture streams for "I'll", "decided to", etc. → candidate queue in daily brief for one-tap confirm. (Partially implemented via tm-reflect)
2. **Voice quick-capture** (15% target): ⌘⇧Space → Whisper-tiny → Tier-1 normalizes. (Not built)
3. **Explicit MCP/CLI** (5% target, highest signal): `memory_commit` / `tracemind commit`. (Shipped)

---

## 11. Outcome attachment (three paths)

1. **Time-triggered prompt** when horizon passes: 👍/👎/🤷 + optional voice/text note
2. **Implicit text matching**: cosine ≥ 0.7 between new captures and open Commitment → proposed Outcome in next brief (shipped in tm-reflect)
3. **Explicit MCP/CLI**: `memory_resolve` / `tracemind resolve` (shipped)

---

## 12. UX surfaces (current and planned)

**Shipped**: CLI (ingest/query/ask/commit/resolve/brief/need/sentiment/action/arc), MCP server (21 tools), Tauri desktop shell (basic query/ingest), capture daemon

**Not built**: daily brief panel, commitment timeline, memory garden graph view, voice mode, capture timeline, surprise panel, settings panel, calibration panel, first-run onboarding, global hotkey ⌘⇧M

---

## 13. Quality benchmarks

### 13.1 LoCoMo results (v0.4, mini-set, 20 questions, 2026-05-09)

| Config | F1 | EM |
|---|---|---|
| v0.2-bge (Tier-0, no extractors) | 25.70 | 5.00 |
| v0.3-bge (Tier-0, threshold tuning) | 25.70 | 5.00 |
| **v0.4-hash / v0.4-bge (Tier-0 + span extractors)** | **49.27** | **30.00** |
| Competitor ceiling (Mem0, full LoCoMo) | 91.6 | — |

Per-category v0.4 (both hash and BGE — extractors are deterministic post-retrieval):
- temporal: 16.67 → **100.00** (recency cue + clock-time extractor)
- adversarial: 25.07 → **73.33** (yes/no oracle with date/money/proper-noun mismatch)
- single_hop: 24.64 → **37.06** (date + money span extraction)
- multi_hop: 36.57 → **39.94** (mild lift from speaker-prefix stripping + Q→A windowing)

The lift comes entirely from `tm-bench-locomo::extract` — a Tier-0 question
classifier (`QKind::{YesNo, Date, Money, Time, Generic}`) and span extractors
that compose tight short-form answers from the retrieved candidate text.
SQuAD F1 punishes long predictions on precision; Tier-0 was returning whole
turns. The extractors close that gap before truncation.

### 13.2 Quality targets

| Metric | Target | Timing |
|---|---|---|
| LoCoMo F1 (mini, Tier-1) | ≥ 60 | Sprint A exit |
| LoCoMo F1 (full, Tier-1) | ≥ 70 | Phase 5A exit |
| LoCoMo F1 (full, all improvements) | ≥ 85 | North star |
| Cross-modal retrieval MAP | ≥ 0.5 | Phase 5A |
| Bitemporal query correctness | 100% (formal) | Phase 5A |

### 13.3 Quality stack (all layers needed for F1 85+)

| Layer | Fix | Est. F1 lift |
|---|---|---|
| Synthesis | Tier-0 → Tier-1 | +20–30 |
| Reasoning | Cross-modal chains + belief-aware ranking | +5–10 |
| Retrieval | Arm 5 (cross-modal) + bitemporal filter | +5–10 |
| Encoding | Multi-modal encoders + shared projection | +3–5 |
| Ingestion | Cross-modal ingest + temporal metadata | +3–5 |
| Storage | Bitemporal + TMS consistency | +2–3 |

---

## 14. Safety and privacy

- **Per-user local, single-command deletable** (`tracemind brain reset`)
- **Inspectable**: `tracemind explain <anticipation_id>` shows grounding
- **Embedding-space only**: world model never generates text about user
- **Calibration-gated**: L2/L3 disabled until accuracy thresholds met
- **Not collected**: location, emotion, biometrics
- **Not built**: predictive UI claiming user state, social layer, cross-user learning, financial/medical recommendation, autonomous action

---

## 15. Engram product (Phase 2)

Problem: current agent memory (Mem0, Letta, Zep) are vector stores with timestamps — no bitemporal time-travel, no retraction propagation, no contradiction detection.

Engram exposes TraceMind's engine as infrastructure for agent developers. Belief-native, not document-native.

API: `assert(belief, evidence)`, `retract(belief_id, reason, evidence)`, `world_at(valid, known_as_of)`, `contradictions()`, `history_of(entity)`.

| TraceMind | Engram |
|---|---|
| Need | Goal |
| Sentiment | Confidence (evidence strength) |
| Commitment | Belief (no horizon/stakes, adds justification DAG) |
| Action | AgentAction |
| Outcome | Observation |
| PatternDetector | ConsistencyChecker (TMS over belief graph) |

Distribution: MCP server, crates.io, PyPI (`engram`), npm (`@tracemind/engram`).

---

## 16. Rosetta product (Phase 3)

CLI + IDE extension that extracts intent from code (code + tests + docs + PRs + commits), preserves intent through transformations (refactoring, migration), proves preservation via property-based tests, and tracks intent drift via bitemporal tracking.

New crates: `tm-semcode` (tree-sitter parsing, intent extraction, semantic diff), `tm-rosetta` (CLI).

| TraceMind | Rosetta |
|---|---|
| Need | Requirement |
| Sentiment | TechDebtSignal |
| Commitment | CodeIntent |
| Action | Implementation |
| Outcome | TransformationResult |

---

## 17. MCP tools (21 shipped + planned)

**Shipped**: memory_store, memory_store_structured, memory_query, get_trace, list_procedures, memory_reason, memory_analogies, memory_consolidate, memory_commit, memory_resolve, memory_brief, memory_insight_silence/unsilence/silences, memory_pattern_silence/unsilence/silences, memory_world_calibration, memory_outcome_proposals/accept/dismiss, memory_need, memory_sentiment, memory_action, memory_arc

**Not built**: memory_recommend (L3), memory_believe, memory_retract, memory_world_at, memory_contradictions, memory_cross_modal, code_intent, code_semantic_diff

---

## 18. Data layout (`~/.tracemind/`)

```
~/.tracemind/
├── memory.db               # SQLite graph + vector
├── intents.db              # Intent system (commitments, outcomes, needs, sentiments, actions)
├── traces.jsonl            # Immutable trace log
├── recent.jsonl            # Capture ring buffer
├── bandit.json             # UCB1 state
├── linucb.json             # Contextual bandit state
├── world_model.json        # f_outcome weights
└── models/                 # Bundled or downloaded
    ├── bge-384-v1.5/
    ├── mxbai-colbert/
    └── gliner-*-multi/     # Optional
```
