# TraceMind — Consolidated History Tracker

**Date:** 2026-07-23
**Purpose:** Preserve historically load-bearing content from strategy/architecture/sprint docs that were deleted during the 2026-07-23 docs cleanup. Anything in this file is *lineage* — the canonical current docs are listed in §0 below. If a claim here conflicts with a current doc, the current doc wins.

---

## 0. What is canonical *now* (not in this file — read those directly)

| Concern | Canonical doc |
|---|---|
| Architecture, cognitive map, intent arc | `docs/DESIGN.md` |
| H2 2026 planning (Jul-Dec) | `docs/CHARTER-H2-2026.md` |
| Active task list (operationalisation) | `docs/TASKS.md` |
| F1 improvement roadmap | `docs/F1_IMPROVEMENT_PLAN-2026-07-22.md` |
| Product experience roadmap | `docs/PRODUCT_EXPERIENCE_PLAN-2026-07-22.md` |
| Ingestion experience + safety roadmap | `docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md` |
| Deferred AIE 2026 backlog | `docs/AIE_2026_BACKLOG.md` |
| Honest self-critique (Jul 2026) | `docs/REVIEW-2026-07.md` |
| Wiring audit (Jul 2026) | `docs/H2-AUDIT-2026-07.md` |
| MVP status + honest numbers | `docs/MVP-STATUS-2026-07.md` |
| Product review + strategic reset | `docs/HOLISTIC-REVIEW-2026-07.md` |
| Post-review implementation report | `docs/HOLISTIC-REVIEW-IMPLEMENTATION-2026-07.md` |
| Host integration guides | `docs/{CLAUDE_CODE,CLINE,CURSOR,GOOSE}_INTEGRATION.md` |
| Design partner check-in script | `docs/DP_CHECKIN.md` |
| Blog | `docs/blog/2026-07-21-tracemind-self-improving-memory.md` |

---

## 1. From `docs/PROJECT_2026.md` (2026-05-11) — previous canonical strategy

Superseded by `CHARTER-H2-2026.md` for H2 planning, but the mission statement and "what we are NOT building" frame remain the strategic spine.

**Mission (still current):**
> Ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded.

**Five positioning properties (still current):**
1. Persistent across every tool (one memory, all hosts).
2. Ambient capture — clipboard, shell, screenshots, browser, audio, calendar; opt-in per source; on-device.
3. Context-aware, not context-rigid — cross-context bridges only fire on learned confidence + surviving feedback.
4. Local-only, on-device — trust is architectural, not promised.
5. Bitemporal + contradiction-aware — retracts stale facts and surfaces them.

Properties 1-4 = wedge. Property 5 = moat.

**"What we are NOT building" — frame discipline (worth restating):**
- Not a "second brain" / Obsidian competitor.
- Not a Rewind-style always-on screen recorder.
- Not a cloud-attached saved-prompts feature (ChatGPT Memory / Mem0 pattern).

**Two-surface strategy (predates but consistent with H2 charter):**
- Surface A: MCP into Claude Code / Goose / Cline / Cursor (primary).
- Surface B: Tauri desktop app (P5, demoted from headline surface).

Head-to-head video was defined as "same host, different memory MCPs" — TraceMind vs Mem0/Letta/Zep inside `claude`.

**North star (seed gate):**
1. One named design partner using TraceMind daily, on camera.
2. Retention: W2 ≥ 40% across ≥5 users.
3. Head-to-head video vs Mem0/Letta/Zep with three beats: persistence, context isolation, retraction.

This is unresolved as of 2026-07-22 (`HOLISTIC-REVIEW-2026-07.md` §0: "0 design partners with real sessions").

---

## 2. From `docs/PRODUCT_PORTFOLIO.md` (2026-05-07) — three-product portfolio

Superseded operationally by focus on TraceMind alone in H2, but the framing remains a reference for future Engram / Rosetta spin-outs.

**Intent arc (still current, embedded in DESIGN.md):**
```
Need → Sentiment → Commitment → Action → Outcome
WHY    HOW I FEEL   WHAT I       WHAT I    WHAT
       ABOUT IT     PLAN TO DO   DID       HAPPENED
```

**One engine, three products (deferred to 2027+):**
| Product | Wedge | Surface | Status |
|---|---|---|---|
| **TraceMind** | System of intents — brief, retraction, outcome loop | Tauri + CLI + MCP | shipping (H2 focus) |
| **Engram** | Agents that *remember* across sessions; contradiction-aware; bitemporal | `cargo add engram` / `pip install engram` | deferred |
| **Rosetta** | Semantic code memory over a repo | team product | deferred |

**Shared engine primitives (all still exist as crates):**
`tm-graph` (bitemporal KG), `tm-tms` (justification-based truth maintenance), `tm-temporal` (point-in-time queries), `tm-intent` (commitments/outcomes/patterns), `tm-reflect` (brief + insights), `tm-world-model` (outcome prediction).

---

## 3. From `docs/UNIFIED_ARCHITECTURE.md` (2026-05-07) — engine crate map

**Three capabilities that were added on top of the base engine:**
1. **Cross-modal reasoning** → `tm-modal` (~2000 LOC)
2. **Bitemporal belief management** → `tm-temporal` (~1500 LOC) + `tm-tms` (~1500 LOC)
3. **Code-intent semantics** → `tm-semcode` (~3000 LOC)

Product-specific surfaces reserved (never built): `tm-engram`, `tm-rosetta`.

Current crate count per `CLAUDE.md`: 30-crate workspace (17-crate map documented). Engine map has grown organically past what this doc captured.

---

## 4. From `docs/PHASE5_CONVERGENCE.md` (2026-05-07) — quality stack

Superseded by `DESIGN.md` and by GEPA-based self-improvement in H2 charter. Historical baseline numbers:
- Phase 5 baseline: **F1 25.70** (Tier-0 only).
- North star at the time: **F1 85+** with Tier-1/Tier-2 answer stack.
- Current: F1 50.32 on fresh conversations (`locomo-train`); 70.49 on `locomo-mini` held-out. See `MVP-STATUS-2026-07.md` for the definitive numbers.

Phase 5's core insight — **make the engine deeper, not wider** — remained the correct instinct. Bitemporal + belief-native + cross-modal are all in the current crate map.

---

## 5. From `docs/SPRINT_GRAPH.md` (2026-05-13) — 5-layer graph stack

Sprint completed. The 5-layer stack survives as the mental model for `tm-graph`:

```
L5 — Composition Layer     (graph algebra over subgraphs; cross-AI MCP export)
L4 — Personal Ontology     (types / verbs / gate on writes)
L3 — Event / Trajectory    (Glean-style event graph; Precedes edges)
L2 — Semantic KG           (entity-SVO triples; canonicalization sidecar)
L1 — Episodic              (raw captures, signals, traces)
```

Investor-flagged gap: **Salience as its own 5th-alongside layer** (not L5 above; distinct concept). Captured in `AIE_2026_BACKLOG.md` §3.6.

---

## 6. From `docs/SPRINT_CTX_EVG_C.md` (2026-05-22) — Outcome bond / Commitment Ledger

Sprint completed. Key architectural conclusion that outlived the sprint:

**Commitment → Outcome loop is what makes the graph feel two-dimensional.** Without it, the second visit to TraceMind looks like the first. `Resolves` edges + Commitment Ledger were the fix. This is the substrate the H2 charter's "Reflexion log" and "GEPA reward signal" now read from.

Prompt UX for "does this outcome resolve a commitment?" is where the sprint said it would live or die. Post-audit (`HOLISTIC-REVIEW-2026-07.md`): the loop is live but has near-zero real-user firings because there are 0 design partners. **The wedge is unvalidated, not unbuilt.**

---

## 7. From `docs/FEEDBACK_LOOP.md` (2026-05-10) — pre-GEPA design memo

Superseded by H2 charter Pillars 1-2 (GEPA + ComposedIndex) and Pillar 7 (recursive self-improvement).

**Insight that persisted into GEPA:** *We get better by scaling feedback, not by scaling capture.* Every re-ask, reformulation, wrong click, mid-query abandonment is a training signal — logged in `traces.jsonl`, `trajectory_store`, `recent.jsonl`. Prior to GEPA, none of it changed behavior beyond immediate bandit reward.

**Positive-signal design that GEPA absorbed:**
- Reward signal fabric: explicit + implicit + behavioral.
- Per-user Pareto frontier (never leaves device).
- Bounded-search over `(instruction text, few-shot demonstrations, weights)` on a small local eval set.

Referenced as ancestor of the current `tm-gepa` crate. The full design is deprecated — see `CHARTER-H2-2026.md` Q4.1-Q4.4 and Q4.11-Q4.14 for what actually shipped.

---

## 8. From `docs/PRODUCT_CLOSE.md` — one-pager for demo close

Retained for messaging reference. The one-line pitch that stuck:

> **ONE ENGINE. THREE PRODUCTS.**
> TraceMind — personal memory OS (shipping)
> Engram — memory SDK for agents (deferred)
> Rosetta — semantic code memory (deferred)
> Local-only by default. Fine-tuneable on-device.

Distribution line: `curl tracemind.dev/install.sh | sh`. No login, no cloud.

---

## 9. From `docs/DEMO_SCRIPT.md` — 3-minute recordable demo

Superseded by MCP-first positioning (Tauri demoted to P5). The **retraction beat** remains the demo hook:

> TraceMind shows the user a contradiction it found in their own knowledge graph and lets them resolve it. This is the only thing on screen that no other "AI memory" product on the market can do.

Demo pre-roll: `tracemind demo restore` + `tracemind demo preroll` (fixture: 60 entities, 75 triples, 1 pre-loaded contradiction). Fixture path: `crates/tm-cli/fixtures/demo/`.

Whenever the demo is re-recorded for MCP hosts (Claude Code first), reuse the fixture and the retraction beat.

---

## 10. From `docs/INVESTOR_DECK.md` (2026-05-06) — investor brief

Superseded by `HOLISTIC-REVIEW-2026-07.md` positioning and by `docs/blog/2026-07-21-tracemind-self-improving-memory.md`. Persistent framing:

- **One-line:** TraceMind is a local-only memory OS for humans and AI agents. Captures intent, predicts outcomes, earns trust on-device.
- **Wedge sentence:** We are not a "memory database." We are a **system of intents.**
- **Competitive frame:** Mem0, Honcho, Letta, Zep, SuperLocalMemory solve forgetting by sending your data to their cloud. Memory is about to be the most sensitive surface in your digital life; the data moat goes the wrong direction.
- **Ask (May 2026):** thinking time + intros, not a check.

The physical decks (`docs/INVESTOR_DECK.pdf`, `docs/TraceMind_Investor_Deck.pptx`) remain in the repo.

---

## 11. Historical baseline numbers (for regression checks)

| Milestone | Date | F1 (locomo-mini) | Notes |
|---|---|---:|---|
| Phase 5 baseline | 2026-05-07 | 25.70 | Tier-0 only, pre-Tier-0 spans |
| v0.4 with Tier-0 spans | 2026-05 | 49.27 | hash == bge (revealed dead retrieval path) |
| GEPA overfit result (retired) | 2026-07 | 75.49 | **do not cite** — tuned on the eval set |
| Held-out `mini` | 2026-07-22 | 70.49 | BGE embedder, honest number |
| Held-out `mini`, hash | 2026-07-22 | 48.33 | separation gate shows BGE lift |
| Fresh `train` | 2026-07-22 | **50.32** | **plan against this** |

---

## 12. Deleted files this cleanup removed

Cross-reference: if you follow a broken link, this is where it went.

- `docs/PROJECT_2026.md` → §1 above
- `docs/PRODUCT_PORTFOLIO.md` → §2
- `docs/UNIFIED_ARCHITECTURE.md` → §3
- `docs/PHASE5_CONVERGENCE.md` → §4
- `docs/SPRINT_GRAPH.md` → §5
- `docs/SPRINT_CTX_EVG_C.md` → §6
- `docs/FEEDBACK_LOOP.md` → §7
- `docs/PRODUCT_CLOSE.md` → §8
- `docs/DEMO_SCRIPT.md` → §9
- `docs/INVESTOR_DECK.md` → §10

Kept but flagged as older process docs: `docs/DP_CHECKIN.md` (live script), `docs/TASKS.md` (references removed `PROJECT_2026.md` — needs a follow-up edit).
