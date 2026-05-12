# Project 2026 — TraceMind

**Authored 2026-05-11. Canonical strategy doc; TASKS.md operationalises it.**

This document is the company's strategic spine for the rest of 2026. It is written from two perspectives — a senior ML engineer auditing the technical state honestly, and an expert VC stress-testing the business case. The plan that follows is the synthesis.

---

## 1. Mission

> **Ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.**

Memory is the single feature missing from every AI assistant shipping today. Cloud incumbents (ChatGPT, Gemini, Claude.ai) bolt memory on as a per-product feature; cloud-first memory startups (Mem0, Letta, Zep) bolt the architecture on as a service you upload your life to. TraceMind is the only stack designed from the ground up around five properties — and no competitor hits all five:

1. **Persistent across every tool** — one memory, reached from Claude Code, Goose, Cline, Cursor, the CLI, the Tauri app. New sessions are not cold starts.
2. **Ambient capture** — clipboard, shell history, screenshots, browser, audio, calendar — all opt-in per source, all on-device. Users do not "use" the memory; the memory is *already populated* by the time they ask. Manual feeding is friction; ambient capture is the wedge moment. Permissions are per-source, revocable, visible.
3. **Context-aware, not context-rigid** — contexts (work, personal, project-A) stay cleanly separated by default; cross-context bridges only fire when learned confidence exceeds a per-arm threshold *and* survive feedback. When the user is in the wrong context (low engagement, repeated `not_related`), TraceMind proposes a switch rather than forcing a bridge. The investor critique on local-machines-have-MORE-context-blur made this a primitive in Sprint C-0; the feedback loop (F-1..F-5) is what makes it adaptive instead of brittle.
4. **Local-only, on-device** — every fact lives in `~/.tracemind/`. No telemetry, no cloud upload, no compliance review. Trust is architectural, not promised.
5. **Bitemporal + contradiction-aware** — every fact has a *valid time* and a *transaction time*; when reality changes, TraceMind retracts the stale fact and surfaces it to the user. This is the architectural moat that protects properties 1–4 from being commoditised by a cloud competitor.

Properties 1–4 are the *wedge* (what makes a user say "I won't go back" in week 1). Property 5 is the *moat* (what makes the wedge defensible against Mem0/Letta/Zep eventually copying the local-first stance).

**Performance is part of the wedge, not a footnote.** If cold start > 2s, if first query after auto-capture is sluggish, or if indexing falls behind the capture stream, the wedge collapses — users feel the friction before they feel the magic. Every quarterly milestone in §6 has a corresponding latency/throughput budget in §7.

---

## 1a. What we are NOT building (frame discipline)

External readers consistently misread TraceMind as one of two products it is *not*. Both misreadings are commercially fatal because they map onto graveyards. Stating the disclaim here in writing keeps the team and any future investor conversation on the right frame.

**TraceMind is not "everything in one" / consumer life-memory.** We are not Rewind, not Personal.ai, not Khoj-for-grandma. The pitch *"your Slack thread references your calendar references your screenshotted restaurant"* is the end-state prize for a hypothetical 2030 product; pursuing it as v1 is the trap that has killed every prior attempt. Twenty connectors × six weeks each is a year of plumbing before the first line of memory logic ships. Entity resolution at consumer tier (Sarah-from-Slack ≡ Sarah-Chen-from-Gmail ≡ S 🌸) is unsolved at the SOTA frontier, let alone on a 1.5B local model. Storage of five years of screenshots + mail + messages dies on phone. We do not pursue this frame.

**TraceMind is "ambient memory for every AI agent you use."** The user-facing surface (Tauri flagship) is a side-effect of that thesis, not the thesis itself. The substrate is populated by the agent ecosystem via MCP — Claude Code reads code, Cursor reads IDE state, Goose reads whatever it points at, future hosts (ChatGPT desktop, Gemini agentic, Slack MCP) bring email/chat as they ship. *We do not build twenty connectors; the agent ecosystem brings ingestion to us.* This is the strategic inversion that makes the local-only constraint tractable where it kills every consumer-shaped attempt.

**TraceMind is not a search platform.** Search-as-killer-feature is underwhelming because Spotlight, Cmd+F, and any LLM with grep already do it. Our surface is *anticipatory* — the right memory surfaced in the context of current work, before the user thinks to ask. Search is the fallback when anticipation misses. See §1b for the anticipatory surface architecture.

**TraceMind is not a productivity-app commitment tracker.** Commitments and contradictions are *primitives inside* the anticipatory surface — they fire when the current work touches an open deadline or a stale fact. They are not the top-level UI. The Brief view and Next Actions panel ladder up to "what should I do given what I'm working on right now," not "here is your to-do list."

If a feature, a roadmap line, or an external pitch reads more naturally as one of the four disclaimed products, it is wrong and should be cut or reframed.

---

## 1b. The anticipatory surface (proactive, not reactive)

The hardest unsolved problem in personal memory is not storage, retrieval quality, or contradiction detection — those are tractable. It is **surfacing the right memory at the right moment without being asked.** Every product that has tried this has failed by being either too noisy (Clippy, every "AI suggestion" panel) or too inert (search bars no one opens). The wedge moment is the *first time* the user sees a card appear that says "you decided X about this last Tuesday" while they are actively re-encountering the same shape of problem — and they did not type a query.

**Architecture: the Working Memory Engine (WME).**

The WME is a rolling-window topic detector + proactive retrieval loop that runs on the capture stream. The contract:

1. **Detect current topic** — maintain a decaying rolling embedding over the last 5 minutes of capture events (clipboard, shell, AI turns, query history, active context). Topic vector updates every 30s or on significant capture event.
2. **Retrieve candidates** — query the memory substrate with the topic vector; pull top-K relevant prior memories that the user has *not* already seen this session.
3. **Score for proactive surfacing** — `score = relevance × surprise × recency_of_decision × outcome_signal`. Surprise filter is critical: do not re-surface what the user is currently looking at; do surface what they have forgotten that bears on it.
4. **Synthesize verb-first cards**:
   - **Resume** — abandoned threads that match current topic (staleness × relevance)
   - **Recall** — prior decisions about the exact thing now in scope
   - **Compare** — analogical matches from `tm-reason::analogy` filtered by current topic
   - **Caution** — negative-outcome patterns from trajectory store (last time you tried this shape, it reverted in 2d)
   - **Connect** — entity → commitment / entity → person joins triggered by current mention
   - **Anticipate** — time-of-day × topic patterns (you usually open file X next when working on Y)

**Commitment and contradiction are special cases of the WME, not the top-level surface.** A commitment card fires when current topic intersects an open deadline. A contradiction card fires when current topic surfaces memories that disagree. Today's Next Actions panel is wrong — it shows every open commitment flat, which is why it reads as noisy. After WME, the Next Actions panel shows only the commitments and contradictions *relevant to what the user is currently doing.*

**This maps to the L1/L2/L3 predictive layers from the April strategic direction note:**
- L1 (detect current intent) — rolling topic vector from capture stream
- L2 (predict useful priors) — retrieval driven by topic vector instead of explicit query
- L3 (surface) — verb-first card synthesis into Brief / Next Actions / MCP `memory_brief`

L1 plumbing exists (capture daemon). L2 exists (retrieval engine). L3 verb cards exist but fire reactively (deadline pass, contradiction detected) rather than predictively. Wiring the WME loop is a Q3 sprint, gated on the seed-critical Q2 work (auto-capture coverage + Tier-1 default).

**Performance budget**: WME tick ≤ 50ms p95 on a quiet laptop; surfacing latency from capture event → card visible ≤ 2s. WME never blocks query path; it is a background producer to a card queue.

**Anti-spam discipline**: per-session displayed-card set; cooldown per (card_kind, target_id); negative-feedback signal `not_useful_now` retrains the score threshold. If WME generates more than 8 cards/hour in steady state, the wedge collapses into Clippy — the threshold tuner is the difference between "magic" and "annoying."

**Engineering substrate (Rust-native, no Python sidecar in the request path).** The WME is not a separate system from the context graph — it is a *consumer* of three storage primitives that index the same capture event stream from different angles:

| Primitive | Crate | Indexes | Powers verbs |
|---|---|---|---|
| Typed-edge graph | `tm-graph` (existing) | relational structure (who/what/when, IsA/WorksAt/PartOf) | Compare, Caution, Connect |
| Vector / ANN | `tm-vector` (existing) + HNSW (PERF-3) | semantic similarity | Recall, Resume |
| **Topic clusters + communities** | **`tm-cluster` (new, Q3)** | density structure on the capture stream + Louvain communities on `kg_entities` | Resume, Recall, Connect, Anticipate |

`tm-cluster` is **HDBSCAN-first, no hardcoded k** (revised 2026-05-12 after partner feedback that KMeans' fixed k is the wrong shape for personal memory — topics emerge organically and unevenly). Substrate stack:

- **Primary clusterer: HDBSCAN** via the [`hdbscan`](https://crates.io/crates/hdbscan) crate (pure Rust port of sklearn HDBSCAN). Density-based, no `k` parameter, auto-discovers hierarchy + emits **outlier points** as `cluster_id = -1`. Outliers are first-class signal: they are the "this doesn't fit anything yet" memories the WME should surface as **Anticipate** candidates.
- **Fallback if `hdbscan` crate maturity is insufficient at integration time**: Python sidecar (sklearn HDBSCAN) called nightly *off the request path* via PyO3 or subprocess. The clustering pass is non-blocking — Tier-0 retrieval continues to work without fresh clusters. Decision deferred to CLU-1 evaluation.
- **Streaming protocol** (since HDBSCAN is batch-oriented): re-cluster the full event set every 100 events OR every 10 min, whichever first. Between re-clusters, new events are *assigned* to the nearest existing cluster centroid (or marked outlier if distance > threshold) but don't trigger re-clustering. The full HDBSCAN pass runs incrementally on a background thread; the user never sees a stall.
- **Storage**: `clusters(id, label_text, centroid_blob, n_members, persistence, is_outlier_bucket, updated_at)`, `event_clusters(event_id, cluster_id, membership_prob, is_outlier)`. The `persistence` field is HDBSCAN's cluster-stability score — higher persistence ⇒ higher confidence the cluster is real.
- **Confidence-routed graph build (highest-confidence first)**: cluster persistence + per-triple SML confidence (§1c P5b) both feed a single `graph_build_confidence` score per relation. Only triples above threshold enter `kg_relations`; below-threshold stay in `pending_relations` for user confirmation. Partner mandate: *"context graphs are built with the highest confidence."*
- **Community detection retained in `tm-graph`**: Louvain over `kg_relations` nightly via `consolidate`. Annotates `kg_entities` with `community_id`. Two-layer organization: HDBSCAN clusters on the *event stream* (topical/temporal), Louvain communities on the *entity graph* (relational). Both are useful, both ship.
- **Outlier detection retained as first-class signal**: HDBSCAN noise points feed the WME's `Anticipate` verb (novel topics) + a Tauri "Unsorted" tray the user can triage. Outliers are not failures — they are forward-leading signal.

The L1 topic vector is the EMA of recent cluster centroids weighted by recency and `membership_prob` — outlier events contribute their own embedding directly (no centroid). Cheap (~384-dim vector update per capture, no model inference).

**Why not KMeans?** Hardcoded `k` is the wrong shape for personal memory — the partner was right. Topics emerge unevenly (some sessions are tightly focused, others scattered), and KMeans forces a uniform partition that papers over real density structure. HDBSCAN matches the underlying distribution; outliers become signal instead of being absorbed into the nearest centroid.

**Why not UMAP / BERTopic stack?** UMAP only via experimental `annembed`. We don't need dim-reduction for clustering at 384-d on 10k–100k events; HDBSCAN handles it directly. BERTopic is a Python orchestration of (UMAP + HDBSCAN + c-TF-IDF) that we replicate in Rust as needed (the c-TF-IDF labeling pass is ~50 LOC). `evoc` (TutteInstitute Rust port of HDBSCAN-on-embeddings) is tracked as a possible swap-in but unstable as of 2026-05.

All three primitives are Rust-native first (linfa for utilities, `hdbscan` crate for clustering, `petgraph` for community detection), CPU-only, and stay in the SQLite footprint. Python sidecar is the *fallback* for clustering if the Rust crate disappoints, and the *only* path for QLoRA finetune (deferred per P12), both off the request path.

---

## 1c. The legibility pillar (visible, granular, steerable memory)

Added 2026-05-12 after partner conversation. The WME (§1b) makes memory **anticipatory**; this pillar makes memory **legible**. Together they form the product narrative: *"memory you can trust because you can see and control it, that also surfaces what you need without asking."*

The wedge moment for legibility is the user clicking on an entity in the Tauri app and seeing — without any manual linking on their part — every memory that mentions it, every relation that touches it, and every conversation thread it appeared in. Obsidian-style backlinks, but with zero manual `[[wiki-linking]]`. The extraction does the linking; the user does the trusting.

**Five legibility primitives (each maps to a P5 sub-priority):**

1. **Obsidian-parity auto-graph + Karpathy-style PKM, zero manual intervention** (P5a) — the kg is already built automatically by `tm-ingest`. What's missing is the *Obsidian-feature-set surface*, automated end-to-end. Reference: Andrej Karpathy's well-known PKM workflow over Obsidian (atomic notes + daily notes + tags + backlinks + [[wikilinks]] + search-driven recall). The bet: ship the *same feature surface*, but *every link is auto-extracted* — the user never types `[[`, never tags manually, never builds a Map of Content. Features to mirror, all auto-populated:
   - **Backlinks panel** — "12 memories link here," auto-derived from `kg_relations`. (Q3, headline)
   - **Bidirectional [[wikilinks]] inline** — entity mentions in memory text auto-render as clickable links to the entity drawer. SML extraction (P5b) drives entity resolution. (Q3)
   - **Transclusion / memory embeds** — one memory can embed another by reference; the embedded memory renders inline. Auto-triggered when a memory is summarised by another. (Q3)
   - **Auto-tags** — hashtags extracted from text + topic-cluster labels from `tm-cluster` HDBSCAN cluster_id. User never types `#`. (Q3)
   - **Daily-notes auto-generation** — `tm-reflect` already produces a brief; promote it to a first-class "Daily Note" memory with date-stamped backlinks to every memory created that day. (Q3)
   - **Maps of Content (MOCs)** — auto-generated index pages per HDBSCAN cluster and per Louvain community. "Everything about Rondo," "Everything about Sequoia conversations" — populated from the graph + clusterer, refreshed nightly. (Q3)
   - **Force-directed graph view** — Q4 polish, never the headline. Anti-spam: cluster-summary view kicks in above 500 nodes.
   - **Canvas / whiteboard** — visual spatial board where the user drops a chosen subset of memories. This *is* the user-controlled splice surface (see §1c primitive #3). (Q4)

   The strategic claim: Obsidian's wedge is a *power-user manual PKM*. Karpathy and a tiny minority use it well; everyone else bounces because the manual linking tax is real. TraceMind's wedge is *Obsidian for everyone* — the same surface, no manual tax. Do **not** clone Obsidian's UI literally; clone the *feature contract*.

2. **Granular open-vocabulary triple extraction** (P5b) — replace static `{IsA, WorksAt, PartOf}` predicates with arbitrary predicate strings extracted by a Small Language Model. Static schema breaks on real-world memory ("Aaditya is critiquing the X paper", "Rondo competes with Veo3"). Candidate SMLs: REBEL (BART-large, 460M), GLiNER-Relation (~150MB), Qwen 2.5 0.5B prompted. Reused Qwen is the cheapest path — same model already auto-downloaded for Tier-1. **Must run off the ingest hot path** (async background worker); heuristic NER stays as the synchronous fast path, SML enriches asynchronously. Reference: partner-recommended guide by Jaya Gupta on automatic-graph-extraction (link to be added when work item opens). Confidence-scored predicates; low-confidence triples don't enter `kg_relations` until validated.

3. **Context splicing — three layers: user-driven, auto-corrective, ontological** (P5c) — partner reframe (2026-05-12): splicing is not just bug-fix for false bridges. Splicing is **giving the user surgical control over which memories enter a given agent session**. Three layers, each independently useful:

   **3a. User-driven splice (the primary feature) — "use these memories, not those"**
   The user must be able to say *"focus on memories 1, 2, 3 for this thread, ignore memory 4."* This is a first-class query API change, not just UI:
   - **Memory Views** — a named, persisted set of memory IDs the user can assemble, save, and reuse. `tracemind view {create, list, add, remove, delete} <name>`. A view is a saved splice.
   - **Per-query inclusion/exclusion** — `memory_query` accepts `{include_ids: [...], exclude_ids: [...], view: <name>}`. CLI: `tracemind query --view "rondo-only" --exclude-ids 42 "what did I decide?"`. MCP tool exposes the same.
   - **Tauri splice UI** — memory list view with multi-select → "Create view from selection" or "Use these for next query." This is the **Canvas/whiteboard surface** from P5a primitive #1 wearing a different hat.
   - **Session-scoped splice** — per-chat-thread splice, ephemeral. Tauri thread sidebar shows the active view; user can edit live.
   - **Export-a-view** — `tracemind export --view <name>` writes the splice as a markdown bundle. Closes the loop with P5d.

   This is the **power-user control surface** that the deny-list can't reach. It also fixes the framing problem with Obsidian: in Obsidian, the user manually curates by writing `[[links]]`; here, the user *post-hoc* curates by picking from auto-extracted memories. Same control, no manual labor up-front.

   **3b. Auto-corrective bridges (the Harry-Potter-↔-Alcatraz fix)**
   Existing CTX-1..CTX-5 (P1d) handles *when bridges should fire*. The HP↔Alcatraz failure (both are "prisons" in embedding space, in disjoint ontological domains) shows we also need *when bridges must NEVER fire*:
   - **Deny-list** (cheap, Q2): explicit `cross_ctx_block_list.json` of (ctx_a, ctx_b) pairs that never bridge. Populated manually or after 3 strikes of `wrong_context_suggestion` feedback on the same pair.

   **3c. Ontological typing (the structural fix)**
   - **Domain classifier** (expensive, Q4): SML pass classifies entities into domains (`fiction.location`, `real.location`, `historical.event`, etc.); deny bridges between entities in disjoint domains regardless of cosine similarity. Closes HP↔Alcatraz at the type level, independent of which contexts the user happens to have created.

   **Splice operations (CLI + UI, Q3)**: `tracemind context {merge A B, split A --by entity X, snapshot A → file}`. Manual splice ops on whole contexts (sibling to Memory Views which splice on memory IDs).

4. **Memory export on demand** (P5d) — `tracemind export --context CTX --format markdown|json|jsonl --output FILE`. Markdown export = portable bundle (one file per entity + index) that opens in any tool, including Obsidian. Tauri "Export this context" button. Entity-scoped audit: "show me everything you know about Pat Grady." Optional PII redaction pass before write. **This is the seed-defensible trust artifact** — local-only stops being a marketing claim and becomes a clickable demo: "here's everything TraceMind has on me, in markdown, on disk, right now."

5. **Cluster-sort UI on `tm-cluster`** (P5e) — Tauri "Memory Garden" view grouping all memories by `tm-cluster::cluster_id` with auto-generated human labels (top-3 entities per centroid). Optional HDBSCAN / evoc swap-in later if dense-region structure matters; don't block the UI on it.

**Sequencing across quarters:**

| Sub-priority | Q2 (seed-critical) | Q3 (post-seed) | Q4 |
|---|---|---|---|
| P5a Obsidian-parity auto-graph | — | backlinks panel + [[wikilinks]] inline + auto-tags + daily-notes + MOCs | force-directed graph view + canvas |
| P5b Granular triples (SML) | — | SML extraction (Qwen 0.5B), async pipeline, confidence-routed acceptance | confidence calibration, schema migration |
| P5c Context splicing | **Memory Views API + CLI** + deny-list + neg-feedback | session-scoped splice in Tauri + `tracemind context` ops | ontological typing |
| P5d Export on demand | **markdown export shipped** (cheap trust win) | Tauri export buttons + view-export + audit-by-entity | redaction toggles |
| P5e Cluster-sort UI | — | **HDBSCAN-driven** Memory Garden + auto-labels + Unsorted (outlier) tray | UMAP/canvas-style spatial view |

**Where this lives in the architecture:** P5 is downstream of `tm-cluster` (P4a) and `tm-graph`. It does not add new storage primitives; it adds (a) a new extractor (SML async pipeline), (b) a new bridge-suppression rule (deny-list), (c) three new UI surfaces (backlinks, export, garden), (d) a new CLI verb (`export`). No re-architecture.

**Performance budget:** SML extraction is async and not on the request path — no latency budget at query time. Backlinks query: ≤ 50ms p95 (it's a single SQL JOIN on `kg_relations`). Export: streamed; no in-memory budget.

**Anti-pattern guard:** legibility ≠ exposure. Visible graph view ≠ raw entity dump. The UI must summarise (cluster labels, backlink counts, top-N relations) before it visualises. The Obsidian graph view fails at >500 nodes; ours hits that in week 2 with auto-capture.

---

## 2. 2026 Vision (where we land by Dec 31)

| Dimension | End of 2026 target |
|---|---|
| Design partners | 50 active, W2 retention ≥ 50% |
| MCP host integrations | Claude Code, Goose, Cline, Cursor — all shipping with quickstart docs |
| Daily active users (MCP wedge) | 2,500 |
| Daily active users (Tauri flagship) | 500 |
| LoCoMo F1 (full set, ~7000 q) | ≥ 65 (cloud-competitive) |
| Cross-session persistence bench (50 pairs) | ≥ 90% |
| Context-aware bench (correct scoping + smart bridge proposals) | ≥ 92% on a 100-pair labeled set (TP bridges − FP bridges) |
| Retraction microbench accuracy | ≥ 85% (moat metric) |
| Ambient capture coverage | ≥ 3 sources live per active DP (clipboard + shell + screenshots minimum), opt-in per source |
| Cold start (CLI / MCP first response) | ≤ 1.5s p95 |
| Auto-capture indexing throughput | ≥ 500 events/min sustained without query degradation |
| Query p50 latency under live capture | ≤ 500ms (Tier-0) / ≤ 800ms (Tier-1) |
| Revenue | $5–10k MRR via Engram developer license (open-core) |
| Funding raised | $1.5M pre-seed/seed extension |
| Headcount | 3 (founder + 1 ML engineer + 1 design/product) |

These are commitments, not aspirations. Every quarterly milestone (§7) ladders to one of these numbers.

---

## 3. ML engineer review (honest technical audit)

### What's actually strong

| System | Why it's good |
|---|---|
| **24-crate Rust workspace** | Real separation of concerns; `tm-types` is dependency-free; ingest/retrieve/rerank/reason/answer all have crate-level boundaries. Audit-grade. |
| **Bitemporal substrate** (`tm-temporal` embedded into `tm-graph`) | This is the single hardest thing to retrofit. Building it from day 1 is a multi-year head start. |
| **JTMS-backed beliefs** (`tm-tms`) | BFS propagation + cosine-trigger contradictions. Closest thing to a *theorem prover for the user's worldview* in the consumer AI space. |
| **MCP server (`tm-mcp`)** | 8 tools, MCP 2024-11-05, stdio JSON-RPC. Already host-compatible. |
| **Feedback loop (F-1 landed)** | Positive + negative signals decompose into the bandit reward. Architectural moat — cloud competitors cannot copy this without uploading user corrections. |
| **Real ONNX embeddings + ColBERT MaxSim rerank** | Not toy embeddings; real BGE-small (384d) + mxbai-edge-colbert late-interaction. Reranker is current SOTA for ≤50MB. |
| **Context segmentation (Sprint C-0)** | Per-context scoping with cross-context penalty + bandit reward decomposition. Solves the local-machines-have-MORE-context-blur misfeature the investor flagged. |

### What's weak, honestly

| System | Diagnosis | Severity |
|---|---|---|
| **LoCoMo F1 = 49.27** | Tier-0 extractive only. Tier-1 (Qwen 2.5 1.5B Q4) is scaffolded but not default. Until Tier-1 ships and we run query rewriting, the headline number is a slide killer. | **Critical — fix by Q2-end** |
| **World model = logistic regression** | `f_outcome` is preflight-only and not actually predictive. The L3 prediction surface promised in the strategic deck does not exist yet. | **Critical for Series A, not for seed** |
| **Bandit (LinUCB, 5 arms)** | 5 arms is too coarse. Real production retrieval systems run 50+ candidate configs and learn fine-grained preferences. UCB1 also converges slowly for cold-start users. | High — fix in Q3 |
| **Reasoning chain depth** | `ChainBuilder` caps at 4 hops; AnalogySolver uses WL kernel (2015 vintage). Modern graph attention or GraphSAGE would lift analogy quality significantly. | Medium — Q4 |
| **No iterative/agentic retrieval** | Single-shot retrieve-then-answer. State-of-the-art systems (Self-RAG, IR-CoT) do multi-step query refinement. Worth +10 F1 on multi-hop. | Medium — Q3 |
| **No HNSW / approximate vector index** | Flat scan over SQLite. Fine until ~50k entities; falls over at 100k+. Long-arc users will hit this. | Medium — Q3 |
| **No cross-modal pipeline** | Images, code, screenshots all referenced in the design but not wired. Cross-modal is the *expansion* story for Series A. | Low for seed; high for A |
| **No proper eval harness with human labels** | LoCoMo is public-set only. We have no human-labeled retraction set, no privacy-leak adversarial set, no longitudinal-coherence set. | High — Q2 |
| **Contradiction detection via cosine < −0.8** | Brittle. Schema-constraint and temporal-overlap triggers are listed in TASKS.md but not landed. Real production needs multiple signals. | Medium — Q3 |
| **Tier-1 = Qwen 2.5 1.5B Q4** | 1.5B params is *small*; reasoning quality will plateau fast. Phi-3.5 (3.8B) or Llama 3.2 3B Q4 are stronger options if RAM budget allows. | Decision needed by Q2-end |
| **No personalization (LoRA / preference learning)** | `tm-llm` packaging + QLoRA sidecar are listed in P8. Without personalization, every user gets the same base model. | Series A blocker |
| **Ambient capture coverage is shallow** | `tracemind-capture` wires clipboard + shell only. Screenshot OCR + VLM caption, browser, audio, calendar, mail — all listed in P10 deferred. But manual-feed is the friction users feel first. Without ambient capture, the "memory just is" promise is empty. | **Critical for seed wedge — promote to Q2** |
| **Cross-context is binary, not adaptive** | C-0 ships hard isolation with a fixed penalty term. No learned threshold per (source_ctx, target_ctx) pair; no context-mismatch detector that proposes a switch; feedback only penalizes, never re-tunes the threshold. Misfeature risk: rigid isolation makes the system feel dumb in genuine cross-context moments (a name mentioned in personal that genuinely matters at work). | **High — fix in Q2-Q3 alongside auto-capture volume** |
| **Performance budget unmeasured** | No tracked cold-start, no indexing throughput number, no query latency under live capture. Auto-capture will push event volume 10-100× and quietly break query-side responsiveness if HNSW + batched embeddings aren't in place. Users feel slow before they feel smart. | **Critical — performance gate from Q2 onwards** |

### Top 7 technical bets for 2026

1. **Ambient capture surface (Q2)** — `tracemind-capture` v2: clipboard (live), shell history (live), screenshot+OCR+VLM caption, browser bookmarklet/extension, calendar import, audio (Whisper-tiny, opt-in). All opt-in per source with revocable permissions, all on-device. Without this, "memory just is" is marketing, not product. Promotes manual-feed to ambient and is the visible wedge to non-developer DPs.
2. **Tier-1 default ship (Q2)** — Qwen 2.5 1.5B Q4 with auto-download, progress UI, query rewriting via paraphrase expansion. Gets LoCoMo F1 to ≥60.
3. **Memory benchmarks (Q2)** — three head-to-heads, public leaderboard at `tracemind.dev/memory-bench`: (a) cross-session persistence (50 pairs, fresh-session recall), (b) context-aware scoping (100 pairs covering correct-isolation + correct-bridge + correct-switch-suggestion), (c) retraction microbench (50 pairs). Score vs. Mem0/Letta/Zep on identical inputs.
4. **Performance gate (Q2 onwards)** — cold-start ≤ 1.5s p95, query p50 ≤ 500ms under live capture, indexing ≥ 500 events/min sustained. HNSW landed earlier than originally planned (Q3, was Q4) because auto-capture volume forces it. Batched embeddings, async indexing, lazy ColBERT rerank gating.
5. **Adaptive cross-context (Q2-Q3)** — learned per-(source_ctx, target_ctx) bridge threshold tuned by `cross_context_bridge` feedback; context-mismatch detector that proposes a switch when retrieval quality is low under the current context. Replaces the current binary penalty.
6. **Bandit scale-up + Iterative retrieval (Q3)** — 5 hand-tuned arms → learned-action-space LinUCB over 20+ retrieval configs; multi-step refinement for complex queries. Feedback loop F-2..F-5 makes this self-improving.
7. **World model v1 (Q4)** — `f_topic` MLP (2-layer InfoNCE, 384→512→384) actually shipped, Platt scaling on `f_outcome`, L1 silent prefetch in background. This is the "predictive memory" slide.

### Top 3 technical risks

1. **Anthropic ships native cross-session memory in Claude Code** — likely 2026 H2. Our MCP wedge gets compressed. Mitigation: lean on the *contradiction* differentiator (which they won't ship for years — it requires a TMS), and use the wedge window to build the Tauri flagship.
2. **Open-source memory framework eats the moat** — Letta is OSS and well-funded. If they ship local-only + contradiction detection, we lose the architectural wedge. Mitigation: ship our retraction microbench publicly *first* and own the benchmark.
3. **Tier-1 quality ceiling on consumer hardware** — Qwen 1.5B may not be enough for the demos that win partners. If true, we either (a) move to Phi-3.5 3.8B and accept 2.5GB footprint, or (b) lean on the *retrieval* moat and don't oversell generation. Decision in Q2.

---

## 4. VC review (honest market audit)

### The category is hot

| Company | Stage | Last valuation | What they do | Local-only? | Contradiction-aware? |
|---|---|---|---|---|---|
| Mem0 | Series A | ~$500M (rumored, 2025) | Cloud memory API for agents | No | No |
| Letta (ex-MemGPT) | Series A | ~$400M | OSS agent memory framework | Optional | No |
| Zep | Seed/A | smaller | GraphRAG-focused memory | No | No |
| Cognee | OSS / pre-revenue | n/a | OSS knowledge graphs for AI | Optional | No |
| Anthropic / OpenAI native memory | n/a (incumbents) | n/a | Built-in to chat products | No | No |

The category is real, well-funded, and growing. None of the funded players ship *local-only + contradiction-aware*. That is TraceMind's moat statement in one phrase.

### Where we win and where we lose

| Dimension | TraceMind advantage | TraceMind disadvantage |
|---|---|---|
| **Privacy / local-only** | Real, defensible moat for a niche (~$X00M TAM: therapists, lawyers, founders under NDA, researchers, journalists, healthcare-adjacent) | Mainstream consumers don't yet care about local-only |
| **Contradiction detection** | Architectural — requires a TMS, which competitors would need months to build | Hard to demo in 10 seconds; needs the right setup |
| **Bitemporal substrate** | Real, would take competitors a year to retrofit | Not visible to users until it's surfaced in UI |
| **MCP-native** | Already shipping into Claude Code + Goose | Every competitor is also shipping MCP servers |
| **Feedback loop (architectural)** | Cloud players cannot copy without uploading corrections | Not yet a slide — needs F-2..F-6 to land |
| **Open source** | Trust signal; community moat | Letta is also OSS and bigger |
| **Two-surface (MCP + Tauri)** | Stronger story than plugin-only or app-only | More to build, slower than pure-play |

### What seed needs (Q3 2026 target)

A defensible seed requires four things. Honest scorecard:

| Seed gate item | Current state | Gap to clear |
|---|---|---|
| 5+ named design partners on camera | 0 | Recruit through Claude Code + Goose communities, Q2 |
| W2 retention ≥ 40% | unmeasured | Ship DP-3 instrumentation, Q2 |
| Persistence-bench ≥ 90% AND context-isolation-bench ≥ 95% (wedge proof) | benches unbuilt | Build benches + run head-to-head, Q2 |
| LoCoMo F1 ≥ 60 OR retraction-bench ≥ 80% (moat proof) | F1 49 / bench unbuilt | Tier-1 default + retraction bench, Q2 |
| Forward 90-day plan with metrics | this doc | ✓ |

### What Series A needs (Q1-Q2 2027 target)

- $50–100k ARR (or 1,000+ DAU with retention curve)
- 6-month retention cohort data
- One viral / press moment (Hacker News #1, TechCrunch, or a Stratechery mention)
- Clear monetization motion (we're betting on Engram developer license — see §6)
- Headcount: 3–5 people

### Top 3 business risks

1. **Cloud incumbents close the gap by bundling.** ChatGPT adds memory for free; Claude adds memory for free; OpenAI ships Atlas with memory. Mitigation: lean on local-only as the *trust* axis (privacy is not bundled away), and ship developer-facing Engram as the wedge that cloud cannot compete on (offline / regulated environments).
2. **No clear monetization.** TraceMind itself is open source + local-only — hard to charge for. Mitigation: monetize through Engram (developer SDK for agent memory in regulated environments — HIPAA, finance, defense) and TraceMind Pro (paid hosted sync for users who want multi-device without giving up E2EE). See §6.
3. **Consumer adoption is brand-from-zero.** A local memory OS for consumers without an installed user base is a long climb. Mitigation: dev wedge → consumer expansion (the Linear/Vercel/Cursor playbook).

---

## 5. The three pillars (Product / Distribution / Business)

### Pillar 1 — Product

**North star metrics (wedge):** persistence bench ≥ 90% AND context-aware bench ≥ 92% AND ambient capture ≥ 3 sources live per DP by Q3. These three numbers quantify what the user feels in week 1.

**Second-tier metric (moat):** retraction microbench accuracy ≥ 80% by Q3, ≥ 85% by Q4. Quantifies the architectural defensibility.

**Seven product priorities, in order:**

1. **The anticipatory surface is the wedge moment.** The Working Memory Engine (§1b) — rolling-window topic detection + proactive retrieval + verb-first card synthesis — is what makes a captured memory *useful* instead of just *stored*. Without it, TraceMind is a search bar with a fancier backend. With it, the user sees "Recall: you decided X about this last Tuesday" appear while they re-encounter the same problem, without typing a query. Q3 sprint to wire L1→L2→L3 end-to-end; Q4 to tune the anti-spam thresholds against design-partner feedback.
2. **Persistence must be effortless and invisible.** Facts mentioned in one `claude`/`goose`/CLI session must appear in the next, in any directory, with zero ceremony. Onboarding never says "configure the memory" — the memory just is.
3. **Ambient capture must do the heavy lifting.** Most users will not hand-feed memory. By the end of Q2, every DP has at minimum clipboard + shell + screenshot capture running with explicit per-source permissions in the Tauri settings panel. By Q4, browser + calendar + audio (Whisper-tiny opt-in) join. The first run after install populates ~50 entities from the last week of clipboard + shell history alone — the "where did this come from?" moment is the magic. *Capture is also the WME's L1 substrate — the two priorities are coupled.*
4. **Context is adaptive, not rigid.** Contexts are scoped by default (Sprint C-0 primitive). Cross-context bridges fire when learned confidence exceeds a per-arm threshold tuned by `cross_context_bridge` feedback. When retrieval quality is low under the current context (low click-through, repeated `not_related`), TraceMind proposes a context switch *or* surfaces a labeled bridge — the user is never silently confused. Hard isolation is the failure mode of an unlearned system, not the goal.
5. **The retraction beat must be undeniable when it fires — but only when relevant to current work.** Contradiction is the moat sentence in the deck, but flat surfacing of every contradiction is noise. After WME lands, contradiction cards fire only when current topic touches a contradicted fact. Same for commitment "Resolve" cards. This is the difference between "retraction is theater" and "retraction is felt."
6. **Performance is a feature, measured every week.** Cold start ≤ 1.5s p95, query p50 ≤ 500ms under live capture, indexing ≥ 500 events/min sustained, WME tick ≤ 50ms p95. If any of these regresses for 2 weeks, halt feature work and fix. Auto-capture will silently break performance if HNSW + batched embeddings + async indexing don't land alongside it.
7. **The Tauri flagship must feel like an app, not a tool.** Brief panel, commitment timeline, context switcher, calibration view, capture-permissions panel — each one click away. Onboarding to "first meaningful brief" in under 60 seconds. The feedback loop must measurably improve retrieval over a week of use (14-day `EvalReport` lift is a slide).

**Out of scope for 2026:** native mobile apps (iOS/Android SwiftUI/Compose), opt-in encrypted-cloud sync. These are 2027 work. Voice + browser + screenshot are **in scope** as ambient capture sources (previously deferred — moved up due to the wedge requirement).

### Pillar 2 — Distribution

**Two surfaces, sequenced:**

1. **MCP wedge (Q2-Q3 2026)** — ship into Claude Code, Goose, then Cline + Cursor. Recruit design partners from these communities. Cheap, fast, on-laptop.
2. **Tauri flagship (Q3-Q4 2026)** — once the MCP wedge has 5 active design partners and recordable testimonials, intensify Tauri investment. First-run onboarding, brief panel polish, commitment timeline. The Tauri walkthrough is slide 2 of the seed deck.

**Three distribution bets:**

1. **MCP host directory landings** — Anthropic's MCP servers list, Goose's extensions registry, Cline's marketplace. Free organic distribution.
2. **One Hacker News moment** — launch the memory benchmark suite publicly with a write-up: *"Why every AI memory layer forgets across sessions, blurs your contexts, and contradicts itself — and what we built instead."* Three numbers (persistence, context isolation, retraction) + an open repo. Land that in Q3.
3. **Engram open-source release** — developer SDK for adding memory to any MCP agent. Q4 launch. Targets developers who want memory in environments they don't control (HIPAA, defense, finance).

### Pillar 3 — Business

**Monetization thesis:** TraceMind (consumer) is free + open source. Engram (developer SDK) is open-core with a paid enterprise license. TraceMind Pro (optional, paid) adds end-to-end encrypted multi-device sync via the user's own iCloud/Drive — never our servers.

**Three revenue streams (priority order):**

1. **Engram developer license (Q4 2026 onwards)** — open-source SDK, paid commercial license for closed-source enterprise use ($X00–$2k/year/dev). Targets: regulated environments where data cannot leave the device.
2. **TraceMind Pro (Q1 2027 onwards)** — $X/month for users who want multi-device sync, paid LoRA finetune capacity, priority support. Capped at the consumer price point ($5–10/month).
3. **Enterprise pilot (Q2 2027 onwards)** — one design-partner-turned-pilot at a single regulated org (law firm, hospital, defense contractor) buying seats. Five-figure deal as proof of motion before the Series A meeting.

**What we will not do:**
- We will not run a SaaS that hosts user memory. The local-only positioning is the moat.
- We will not raise a $5M seed. Right round is $1–1.5M pre-seed in Q3-Q4 2026 after the seed gate clears.
- We will not pursue the three-product narrative (TraceMind + Engram + Rosetta) in external comms until TraceMind alone hits W2 ≥ 40%.

---

## 6. Quarterly milestones (Q2 2026 → Q1 2027)

### Q2 2026 (May–June) — Seed-gate sprint

**Theme:** ship the artifacts that fill the seed deck.

- [x] MCP host integration docs (Claude Code + Goose) — done 2026-05-11
- [x] One-command installer for Claude Code — done 2026-05-11
- [ ] Goose installer mirror (`scripts/install_goose.sh`)
- [ ] DP-3 instrumentation: `tracemind share-usage` CLI
- [ ] 5 design partners onboarded
- [ ] **Ambient capture v2 (Q2 seed-critical)** — clipboard + shell already wired; **add: screenshot OCR + VLM caption, browser bookmarklet, per-source permission UI in Tauri settings panel.** Capture coverage ≥ 3 sources live per DP. First-run backfill from clipboard + shell history of past 7 days.
- [ ] **Performance baseline + budget tracking** — `tm-bench` covers cold-start, query-under-capture p50/p95, indexing throughput. CI gate: any regression > 10% blocks merge. HNSW vector index landed Q2 (was Q3) because of auto-capture volume.
- [ ] **Adaptive cross-context v1** — learned per-(source_ctx, target_ctx) bridge threshold tuned by `cross_context_bridge` feedback; replaces the fixed C-0 penalty term.
- [ ] Tier-1 default ship (Qwen 2.5 1.5B Q4, auto-download, query rewriting)
- [ ] LoCoMo mini-set F1 ≥ 60 with Tier-1
- [ ] Memory benchmarks: persistence (50 pairs), context-aware scoping (100 pairs incl. correct-bridge + correct-switch-suggestion cases), retraction (50 pairs). Public leaderboard at `tracemind.dev/memory-bench`.
- [ ] UI-7 Tauri walkthrough screen recording
- [ ] UI-8 first-run onboarding flow — must showcase auto-capture populating the brief from clipboard/shell within 60 seconds
- [ ] UI-13 capture-permissions panel — per-source toggle, revoke, "last captured at" timestamp
- [ ] DP-6 first testimonial video captured — must include the "I never typed any of this in — it just knew" moment

**Exit metric:** 5 active DPs + 1 testimonial + 4 defensible numbers (persistence bench, context-aware bench, retraction bench, performance budget) + auto-capture live on ≥ 3 sources per DP + LoCoMo F1 ≥ 60 in reach.

**Legibility Q2 pulls forward (§1c):**
- [ ] **Markdown export shipped** (§1c P5d) — `tracemind export --context CTX --view VIEW --format markdown` writes a portable bundle. Tauri "Export this context" button on Settings. Trust artifact for the deck.
- [ ] **Context deny-list shipped** (§1c P5c-3b) — `cross_ctx_block_list.json` + 3-strike auto-add on `wrong_context_suggestion`. Closes the Harry Potter ↔ Alcatraz objection at the deny-list level.
- [ ] **Memory Views API + CLI shipped** (§1c P5c-3a) — `tracemind view {create, list, add, remove, delete} <name>` + `tracemind query --view <name> --include-ids ... --exclude-ids ...` + MCP `memory_query.view` parameter. Power-user splice control without UI.

### Q3 2026 (July–September) — Seed raise + scaling + anticipatory surface

**Theme:** raise $1–1.5M, scale the wedge, ship the feedback loop, **wire the Working Memory Engine end-to-end.**

- [ ] **`tm-cluster` crate substrate** (§1b engineering substrate): new crate wrapping `linfa-clustering`. MiniBatchKMeans on BGE embeddings of capture events; SQLite tables `event_clusters` + `clusters`; re-cluster trigger every 100 events / 10 min; drift detection via mean-distance threshold → split cluster. Persisted in `memory.db`. Public API: `Clusterer::assign(event_id, embedding) -> ClusterId`, `Clusterer::centroid(cluster_id) -> Vec<f32>`, `Clusterer::recent_centroids(window) -> Vec<(ClusterId, f32)>`. **Gates WME L1.**
- [ ] **Louvain community detection in `tm-graph`** (§1b engineering substrate): ~200 LOC community-detection pass over `kg_relations` (or pull `leiden-rs`). Annotates `kg_entities` with `community_id`. Runs nightly via `consolidate`. **Gates WME L2 "Connect" verb.**
- [ ] **Working Memory Engine (WME) v1 — anticipatory surface** (§1b): rolling topic detector over capture stream (L1 = EMA of `tm-cluster` recent centroids), proactive retrieval driver (L2 = `tm-graph` + `tm-vector` + `tm-cluster` joined query), verb-first card synthesis (Resume/Recall/Compare/Caution/Connect/Anticipate). Commitment + contradiction cards become *filtered* outputs of WME instead of flat Next Actions. Performance budget: WME tick ≤ 50ms p95; capture→card surfacing ≤ 2s. Anti-spam: ≤ 8 cards/hour steady state per session. **Gates Q3 anticipatory-surface milestone.**
- [ ] **WME feedback signals**: `useful_now`, `not_useful_now`, `not_now_remind_later`, `dismiss_this_kind`. Retrain card-score threshold per signal.
- [ ] Run W-1/W-2 head-to-head against Mem0/Letta/Zep in Claude Code — publish 4-pane video
- [ ] Full LoCoMo run (≥ 65 F1 target)
- [ ] F-2..F-5 land: corpus extractor, `TuneConfig` surface, `tm-eval` crate, `tm-tune` GEPA-lite
- [ ] **Adaptive cross-context v2** — context-mismatch detector that proposes a context switch when retrieval quality is low; surfaced in Tauri brief + MCP tool response. Tuned by feedback signals.
- [ ] **Ambient capture v3** — audio (Whisper-tiny, opt-in hotkey), calendar import, code IDE telemetry. All opt-in per source.
- [ ] **Performance scale-up** — iterative retrieval lands without breaking the p50 budget. Indexing sustains 1000+ events/min under DP-realistic capture load. RAM idle ≤ 1.6GB with Tier-1.
- [ ] Cline + Cursor integration docs
- [ ] Hacker News launch: "Why every AI memory layer forgets, blurs your contexts, and contradicts itself"
- [ ] 20 active design partners, W2 ≥ 40%
- [ ] Seed deck v1 → partner meetings → term sheet
- [ ] Hire #1: ML engineer (LoCoMo + bandit + world model + capture pipelines)

**Legibility Q3 lands (§1c):**
- [ ] **Granular open-vocabulary triple extraction** (§1c P5b) — Qwen 2.5 0.5B async background extractor; open-vocab predicates replace static `{IsA, WorksAt, PartOf}`; heuristic NER stays as synchronous fast path. Confidence-routed: high-confidence triples enter `kg_relations`, low-confidence stays in `pending_relations`. Reference Jaya Gupta graph-extraction guide + Karpathy PKM workflow.
- [ ] **Obsidian-parity auto-graph (Karpathy PKM, zero manual)** (§1c P5a) — Tauri: backlinks panel ("12 memories link here"), inline auto-rendered [[wikilinks]] on entity mentions, transclusion of summarised memories, auto-tags from cluster labels, auto-generated daily-note memory, auto-generated MOCs per cluster + per community. **All zero manual intervention** — extraction does the linking, user does the trusting.
- [ ] **Session-scoped Memory Views in Tauri** (§1c P5c-3a) — Tauri thread sidebar shows the active view; multi-select memories → "Use these for next query." Live-editable per chat thread.
- [ ] **Manual context splice operations** (§1c P5c) — `tracemind context {merge A B, split A, snapshot A}` CLI + Tauri UI. Powers curated export bundles for DP onboarding.
- [ ] **HDBSCAN-driven Memory Garden + Unsorted tray** (§1c P5e) — Tauri grid grouped by HDBSCAN `cluster_id` with auto-labels; outlier (`cluster_id = -1`) bucket renders as "Unsorted" tray the user triages. Community-detection labels overlaid.

**Exit metric:** signed term sheet, $1–1.5M raised, hire #1 starts.

### Q4 2026 (October–December) — Engram launch + Tauri flagship maturity

**Theme:** ship the second product surface; ship the first revenue stream.

- [ ] Engram public release (open source + commercial license)
- [ ] World model v1: `f_topic` MLP, Platt scaling on `f_outcome`, L1 silent prefetch
- [ ] **Cross-modal pipeline (pulled forward from Q1 2027)** — SigLIP-small on captured screenshots, tree-sitter on captured code, audio transcripts join the entity graph. Auto-capture pipelines from Q2-Q3 generate the inputs; Q4 wires the cross-modal entity edges.
- [ ] Iterative / agentic retrieval (multi-step refinement)
- [ ] Tauri commitment timeline + calibration panel shipped
- [ ] First Engram commercial deal ($X00–$2k/year/dev)
- [ ] 50 active design partners, W2 ≥ 50%
- [ ] Hire #2: design / product (Tauri UX + onboarding + capture-permissions polish)

**Legibility Q4 polish (§1c):**
- [ ] **Ontological typing for context bridges** (§1c P5c expensive path) — SML pass classifies entities into domains (fiction.location / real.location / etc.); bridges denied across disjoint domains. Closes the Harry Potter ↔ Alcatraz case at the *type* level, not just the deny-list level.
- [ ] **Force-directed graph view** (§1c P5a polish) — Tauri graph view with cluster-coloring, summarised down to top-N entities per cluster. Anti-spam: never render > 500 nodes raw.
- [ ] **HDBSCAN / evoc swap evaluation** (§1c P5e polish) — if Rust HDBSCAN or evoc reaches production grade, swap into Memory Garden for hierarchical density structure. Otherwise keep KMeans.
- [ ] **Triple-confidence calibration** (§1c P5b polish) — Platt-scale the SML's emitted confidence; bandit-style routing decides which triples enter `kg_relations` vs. stay in a pending pool surfaced to the user for confirmation.

**Exit metric:** $2k MRR from Engram, 50 DPs at 50% W2, Tauri walkthrough is a 2-minute polished tour, cross-modal captures live for ≥ 10 DPs.

### Q1 2027 (January–March) — Series A prep

**Theme:** show retention, show revenue, show traction.

- [ ] TraceMind Pro alpha (E2EE multi-device sync)
- [ ] LoCoMo F1 ≥ 70 (with iterative retrieval)
- [ ] First enterprise pilot signed (5-figure)
- [ ] 6-month retention cohort data (charts ready for Series A)
- [ ] Cross-modal expansion: code IDE deep-integration (VS Code / Cursor / JetBrains plugins), audio-conversation summarization, mail/calendar bidirectional sync
- [ ] $10k MRR (combined Engram + early Pro)

**Exit metric:** Series A meetings begin.

---

## 7. Metrics that matter (the dashboard)

Track these weekly. Anything else is noise.

| Metric | Source | Q2 target | Q3 target | Q4 target |
|---|---|---|---|---|
| Design partners (named, active) | manual roster | 5 | 20 | 50 |
| W2 retention (opt-in `usage.json`) | DP-3 | measured | ≥ 40% | ≥ 50% |
| LoCoMo F1 (full set) | `tm-bench-locomo` | 50 mini | 65 | 65 |
| Persistence bench (fresh-session recall) | new `tm-bench-memory` | bench built, ≥80% | ≥85% | ≥90% |
| Context-aware bench (TP bridges − FP bridges, 100 pairs) | new `tm-bench-memory` | bench built, ≥85% | ≥90% | ≥92% |
| Retraction bench accuracy (moat) | new `tm-bench-memory` | bench built | 80% | 85% |
| **Ambient capture coverage (sources live per DP)** | `tracemind status capture` | ≥3 (clip+shell+screenshot) | ≥4 (+browser) | ≥5 (+audio or calendar) |
| **WME proactive-card useful-rate** (cards marked `useful_now` ÷ cards surfaced) | DP-3 feedback log | n/a — WME ships Q3 | ≥30% baseline | ≥45% |
| **WME cards/hour steady state** (anti-spam guardrail) | DP-3 feedback log | n/a | ≤8 | ≤6 |
| **Legibility — context deny-list false-bridge rate** (Harry Potter ↔ Alcatraz class) | new `tm-bench-context` | ≤10% (deny-list Q2) | ≤5% (manual splice + feedback Q3) | ≤2% (ontological typing Q4) |
| **Legibility — SML triple-extraction precision** (open-vocab predicates) | new `tm-bench-triples` | n/a — SML ships Q3 | ≥0.70 precision @ confidence ≥0.7 | ≥0.80 |
| **Legibility — export bundle adoption** (% of DPs who run `tracemind export` ≥1×/week) | DP-3 usage log | ≥40% (markdown Q2) | ≥60% | ≥70% |
| **Cold-start CLI/MCP first response p95** | `tm-bench` | ≤1.5s | ≤1.2s | ≤1.0s |
| **Query p50 under live capture** | `tm-bench` | ≤500ms (T0) / ≤800ms (T1) | ≤400ms / ≤600ms | ≤300ms / ≤500ms |
| **Indexing throughput (events/min sustained)** | `tm-bench` | ≥500 | ≥1000 | ≥1500 |
| MCP host directory listings | manual count | 2 | 4 | 6 |
| Engram commercial deals | manual roster | 0 | 0 | 1 |
| MRR | manual | $0 | $0 | $2k |
| Idle RAM footprint | OS measure | ≤1.6 GB | ≤1.6 GB | ≤1.6 GB |

The first three rows are the seed-gate dashboard. The bolded rows are the **wedge-credibility** dashboard — auto-capture coverage and performance budgets are user-felt and must move every quarter. The last four rows are technical credibility for Series A. **If any bolded metric regresses for 2 weeks, halt feature work and fix.**

---

## 8. Risk register (top 8)

| # | Risk | Likelihood | Impact | Mitigation owner | Mitigation |
|---|---|---|---|---|---|
| 1 | Anthropic ships native cross-session memory in Claude Code | High | High | Founder | Lean into contradiction differentiator + accelerate Tauri flagship to be sticky surface |
| 2 | Cannot recruit 5 design partners by Q2-end | Medium | Critical | Founder | If P0 candidates refuse 3× → wedge is wrong, return to brainstorming |
| 3 | LoCoMo F1 stalls below 60 with Tier-1 | Medium | High | Founder + hire #1 | Move to Phi-3.5 3.8B; ship retraction bench as alternative headline number |
| 4 | Letta ships local-only + contradiction in 2026 | Medium | High | Founder | Own the retraction microbench publicly first; bitemporal as second moat |
| 5 | Tauri app stays demo-quality, not product-quality | High (without hire #2) | High | Hire #2 | Q3 design hire; UI-7..UI-12 must ship |
| 6 | Founder burnout / solo-founder velocity ceiling | Medium | High | Founder | Hire #1 by Q3-end is non-negotiable |
| 7 | Open-source contributors fork the moat | Low | Medium | Founder | Engram commercial license + paid Pro tier ahead of fork pressure |
| 8 | Pre-seed market cools; can't raise $1–1.5M | Low (2026) | High | Founder | Bootstrap longer on Engram revenue; cut burn |
| 9 | **Ambient capture feels creepy → uninstall** | High (without UX care) | Critical | Founder + hire #2 | Per-source opt-in; "captured at" provenance on every memory; one-click "forget this source"; never collect a source the user hasn't toggled on |
| 10 | **Auto-capture volume breaks query latency** | High (without HNSW + async) | Critical | Founder + hire #1 | Performance gate in CI; HNSW landed Q2; batched embeddings; async indexing; throttling capture under load |
| 11 | **Cross-context too rigid → feels dumb** | Medium | High | Founder | Adaptive threshold + mismatch detector lands Q2-Q3; feedback-trained; visible bridge labels so wrong bridges are easy to penalize |
| 12 | **Cross-context too permissive → feels invasive** | Medium | High | Founder | Default threshold conservative; bridges always labeled; `cross_context_bridge` negative feedback halves the threshold permanently for that pair |

---

## 9. Hiring plan

| Quarter | Hire | Role | Why |
|---|---|---|---|
| Q3 2026 | #1 | ML engineer | Owns LoCoMo, bandit, world model. Frees founder for distribution + raise. |
| Q4 2026 | #2 | Design / product | Owns Tauri UX + onboarding + brand. Founder + ML engineer cannot ship product-grade UI alone. |
| Q1 2027 | #3 (optional) | Developer advocate | Only if Engram MRR justifies it. Otherwise defer to post-Series A. |

Founder owns: company direction, fundraising, design-partner relationships, technical leadership through Q3.

---

## 10. Funding plan

| Round | Size | Timing | Use of funds |
|---|---|---|---|
| Friends & family / angel | $100–250k | Q2 2026 (only if needed for runway) | Cover living + AWS credits during seed-gate sprint |
| Pre-seed / seed | $1–1.5M | Q3-Q4 2026 | Hires #1 and #2, 18-month runway |
| Series A | $8–12M | Q2-Q3 2027 | Scale to 10 people, enterprise GTM |

**Investors to target for pre-seed:** the investor who flagged Goose (assuming relationship is good), plus 3–5 local-first / privacy-aligned funds and angels (e.g., people who backed Linear, Cursor, Vercel, Obsidian's enterprise tier).

**Do not pursue:** generic AI Tier-2 funds who'll push us to abandon local-only for SaaS economics.

---

## 11. Operating cadence

- **Daily** — one solo standup: what shipped yesterday, what ships today, what's blocked. Written to `~/.tracemind/standup.md` (dogfooding).
- **Weekly (Friday)** — review the 3 numbers: design partners, W2, F1. Update PROJECT_2026.md if any quarterly milestone slips.
- **Monthly** — design-partner check-ins (15 min each), review of risk register, deck refresh.
- **Quarterly** — full re-read of this doc, update Q+1 milestones, kill or reframe anything that hasn't moved.

The single most important rule: **if the weekly numbers don't move for 4 weeks running, the wedge is wrong.** Stop shipping. Brainstorm. Return.

---

## 12. The bet, in one paragraph

> Every AI assistant forgets, and every cloud memory startup makes you upload your life to a third party. TraceMind is ambient memory for every AI you use — it captures what you do (clipboard, shell, screenshots, browser, audio — all opt-in per source), scopes itself to the right context, learns when contexts genuinely connect, and never leaves your device. One memory, reached from Claude Code, Goose, Cline, Cursor, and the Tauri flagship; populated ambiently so the user never feels they're "feeding the memory"; context-aware by default and feedback-trained so work doesn't bleed into personal *and* genuine cross-context bridges still fire when they should. The architectural moat is a TMS-backed, time-aware, feedback-driven engine that retracts cleanly when reality changes — the contradiction beat no cloud competitor can copy without uploading the user's corrections. Performance is a first-class feature: cold start ≤ 1.5s, query p50 ≤ 500ms under live capture, indexing ≥ 500 events/min sustained. We ship as MCP servers into the agents users already run (the wedge), and as a Tauri flagship where users go to see and manage their memory (the surface). By end of 2026 we have 50 design partners, $5–10k MRR from Engram, and a defensible Series A story. By 2028 we are the default memory layer for the local-first AI stack.

That paragraph is the deck.
