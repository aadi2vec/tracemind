# Sprint CTX-EVG-C — Outcome bond / Commitment Ledger

**Status:** proposed
**Branch:** `p4` → next sprint branch off `main` after EVG Slice A/B merge
**Author:** Aaditya Srivathsan + Claude
**Drafted:** 2026-05-22

This sprint is the **closing arrow** on the system-of-intents wedge. Slices A and B of
CTX-EVG wired ingest/query into the event graph and gave threads a sequence backbone
(Precedes edges promoted on thread end). The graph now has *time* but no *fate*:
commitments are write-only — they get logged and rot. Without a `Commitment → Outcome`
loop, the second visit to TraceMind looks like the first, and the product feels
one-dimensional. This sprint adds the missing dimension.

The schema work is small. **Most of the sprint is the prompt UX** for "does this outcome
resolve a commitment?" If that prompt has friction, the loop dies and the sprint fails.

> Strategic context: see `MEMORY.md → tracemind_outcome_bond_stickiness.md` for why this
> beats hotness/identity facets/confidence gradient as the next stickiness lever.

---

## 1. Where this fits in the 5-layer stack

```
┌────────────────────────────────────────────────────────────────────┐
│  L5 — Composition Layer                                            │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L4 — Personal Ontology                                            │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L3 — Event / trajectory graph        (THIS SPRINT — closes loop)  │
│  Commitment.due_at + Commitment.state + Resolves edge wiring       │
│  Auto-derive 'broken' when due_at < now and no Resolves edge       │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L2 — Personal LGM (consumes outcome history once stable)          │
└────────────────────────────────────────────────────────────────────┘
                              ↓
┌────────────────────────────────────────────────────────────────────┐
│  L1 — Entity + topic substrate                                     │
└────────────────────────────────────────────────────────────────────┘
```

CTX-EVG-C lives entirely inside L3. It does not require ontology (L4) or LGM (L2) — but
once shipped, L2 finally has a real outcome variable to forecast on
(`commitment_completes | topic, weekday, has_decision_ancestor`).

---

## 2. Schema deltas

All changes are additive and run via `CREATE TABLE IF NOT EXISTS` / `ALTER TABLE`
migrations in `GraphStore::open`.

### 2.1 `event_nodes` extension — commitment fields

```sql
ALTER TABLE event_nodes ADD COLUMN due_at INTEGER;           -- unix millis, nullable
ALTER TABLE event_nodes ADD COLUMN state  TEXT NOT NULL DEFAULT '';
                                            -- '' | 'pending' | 'kept' | 'broken' | 'abandoned'
CREATE INDEX IF NOT EXISTS idx_event_nodes_due ON event_nodes(due_at) WHERE due_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_event_nodes_state ON event_nodes(state) WHERE state != '';
```

Rationale: `due_at` + `state` only make sense for `kind = 'commitment'`. Storing them on
`event_nodes` (vs a sidecar table) keeps the read path single-row and lets the existing
EventGraphStore CRUD path service them. Empty-string `state` is the sentinel for "not a
commitment" — partial indexes keep this cheap.

State transitions:
- `pending` — written at commitment ingest
- `kept` — set when a `Resolves` edge is upserted *and* the user (or heuristic) confirms
  positive resolution
- `broken` — set when `due_at < now()` and no `Resolves` edge exists at sweep time
- `abandoned` — set manually by the user from the Ledger UI

### 2.2 `Resolves` edge — already exists

`EventEdgeKind::Resolves` is already in `tm-graph::event_graph`. This sprint *wires* it,
not defines it. The edge is directional: `(Outcome) -[Resolves]-> (Commitment)`. A
`Resolves` edge carries `strength = polarity` ∈ {-1.0, +1.0}:
- `+1.0` → kept (resolved positively)
- `-1.0` → broken (the outcome confirmed the commitment was missed)

A commitment can have multiple Resolves edges if the user updates the outcome later
(idempotent upsert via existing `(from,to,kind,context_id)` uniqueness).

### 2.3 No new tables

Everything fits in `event_nodes` + `event_edges`. The Ledger UI reads via materialized
views over these two tables.

---

## 3. Code structure

### 3.1 `tm-graph` additions

```
crates/tm-graph/src/event_graph.rs
  + EventNode::commitment(payload_ref, due_at) -> EventNode (state='pending')
  + EventGraphStore::set_commitment_state(conn, node_id, state)
  + EventGraphStore::resolve_commitment(conn, outcome_id, commitment_id, polarity)
      ↳ upserts Resolves edge AND flips commitment.state to kept|broken
  + EventGraphStore::sweep_broken(conn, now_ms) -> usize
      ↳ for every commitment where state='pending' AND due_at < now AND no Resolves edge,
        set state='broken'; idempotent
  + EventGraphStore::commitment_ledger(conn, since_ms, until_ms) -> LedgerSummary
      ↳ counts by state in window + returns the visible subgraph node IDs
```

### 3.2 `tm-ingest` — commitment detection

```
crates/tm-ingest/src/commitment.rs (NEW)
  pub fn detect_commitment(text: &str) -> Option<CommitmentCandidate>
```

Heuristic-first (no LLM in hot path):
- first-person futures: "I'll …", "I will …", "I'm going to …", "let me …"
- imperative-to-self: "remind me to …", "todo: …"
- explicit dates: NLP date extraction via `chrono-english` (already a transitive dep) or
  bundled `dateparser`; falls back to "no due date" (`due_at = None`)

If detected, the pipeline writes a `Commitment` event node alongside (not instead of)
the existing `Capture` node. The `Capture` carries the raw text; the `Commitment`
carries the structured intent.

### 3.3 `tm-tauri` commands

```
cmd_commitment_ledger(window_days: u32) -> LedgerDto
cmd_commitment_resolve(commitment_id: Uuid, outcome_text: String, polarity: f32)
cmd_commitment_set_state(commitment_id: Uuid, state: String)
cmd_commitment_propose_resolution(outcome_id: Uuid) -> Vec<ProposalDto>
    ↳ returns top-3 candidate commitments to resolve (entity overlap + recency window)
```

### 3.4 CLI

```
tracemind commit "<text>" [--due <when>]
tracemind ledger [--window 7d]
tracemind resolve <commitment-id> --kept | --broken
tracemind demo evg-ledger      # fixture: seeds 6 commitments, 4 outcomes, 2 missed
```

### 3.5 Frontend

```
crates/tm-tauri/ui/src/
├── views/
│   ├── LedgerView.tsx               ← Commitment Ledger surface
│   └── ThreadsView.tsx              ← (existing) gains a small "ledger" row per thread
├── components/
│   ├── LedgerScoreCard.tsx          ← homepage card: this week's kept/broken/pending
│   └── ResolutionPrompt.tsx         ← the friction-critical prompt
└── api.ts                            ← extend with commitment* wrappers
```

The **Brief** (homepage) gets one new card: `LedgerScoreCard`. It shows three numbers
and a sparkline of the last 4 weeks. Clicking opens `LedgerView`.

`LedgerView` is the full surface: a column for each state with the commitments listed,
the resolving outcome (if any) inline, and an "End-of-week sweep" affordance.

---

## 4. The prompt UX — the only thing that matters

The sprint succeeds or fails on this single interaction. Rules:

1. **Triggered on outcome ingest, not commitment ingest.** When the user records "shipped
   the EVG sprint", we look back ≤14 days for `pending` commitments with entity overlap
   and offer the top 1–3.
2. **One tap to confirm, one tap to dismiss.** If neither tap, the prompt
   self-collapses after 8s and is *not* re-shown for that outcome. Friction = death.
3. **No modal.** Inline pill in the capture toast.
4. **Heuristic auto-resolve at ≥0.85 confidence** — skip the prompt, write the edge,
   show a "kept ✓ (undo)" toast for 5s. Undo flips the state back.
5. **Sweep silently.** `sweep_broken` runs on every Brief load. The user sees
   "3 commitments broke this week" as a passive count — never an interruption.

If we cannot get the prompt to feel like nothing, we ship without the prompt and rely on
the sweep + manual `tracemind resolve`. The Ledger card still ships either way — that
is the surface that earns the second visit.

---

## 5. Acceptance criteria

- `cargo check --workspace` clean (no-default-features path)
- `cargo test -p tm-graph -p tm-ingest` green; new tests cover:
  - `commitment` node insert + state transitions
  - `resolve_commitment` upserts the Resolves edge and flips state atomically
  - `sweep_broken` is idempotent and only flips eligible rows
  - `detect_commitment` recognizes the 4 patterns above on a fixture set
- `cmd_commitment_ledger` returns real counts against a seeded DB; UI renders without
  console errors
- `tracemind demo evg-ledger` produces a non-empty Ledger view end-to-end
- `tsc --noEmit` + `vite build` clean
- Single squash-merge PR titled
  `feat(graph): CTX-EVG-C — outcome bond + Commitment Ledger`

### Stickiness measurement (post-merge)

This is the first sprint where we have a falsifiable retention hypothesis. Instrument:
- D2 / D7 return rate before and after CTX-EVG-C ships (local-only, opt-in counter in
  `~/.tracemind/usage.json`)
- median ledger-card views per active week
- ratio of resolved-via-prompt vs. resolved-via-CLI

If D7 return rate does not move within two weeks of personal dogfooding, the diagnosis
in the strategic memory was wrong and we re-evaluate before any further "stickiness"
sprint.

---

## 6. Out-of-scope

- LLM-based commitment detection (Tier-1 backend integration) — Q3 follow-up; the
  heuristic path must be the floor
- Cross-thread Resolves edges (e.g. outcome in thread B resolves commitment in thread A)
  — allowed by schema, gated by UI; ship same-thread first
- Notifications / reminders before `due_at` — deliberately deferred; this sprint is
  about the *graph closing*, not about productivity-app surface area
- Recurring commitments ("every Monday") — Q3
- LGM consumption of the outcome series — separate `tm-pgm` sprint once we have ≥30
  resolved commitments to learn from
