# TraceMind walkthrough — 2026-05-15

End-to-end walkthrough of every surface shipped in the last 5 days
(May 10 → May 15). All screenshots are live against `/tmp/tm-demo`
(seeded from `~/.tracemind/`: **407 entities, 832 triples, 197 traces,
21 communities**). Captured with the freshly rebuilt Tauri bundle
(`target/release/TraceMind.app/Contents/MacOS/TraceMind`, May 15 10:17).

## Commits covered

```
e31b522  fix(pgm,ui,reflect):  PGM raw-count storage + sidebar IA + proposer dedup
cc0fdc9  feat(evg-followup):   ONT-2 statistical proposer + LGM-2 Anticipate + ontology gate
a0396cc  feat(graph):          Sprint GRAPH — unified 5-layer memory graph  (#47)
5d2bc52  P5 improvements                                                    (#46)
14a245a  Work/p5 audit                                                      (#45)
bc35ca6  feat(legibility):     P5 wrap — LM-3/4/5a/5b/5d/8/11e/15/20/22/23  (#44)
d8bdb1b  feat(legibility):     P5 Tauri surface + tm-bench-triples          (#43)
16746b3  feat(legibility):     P5 backlog — LM-5c/7/9/9b/13/14/17-be        (#42)
```

## TL;DR — what's now live

| Surface           | Shipped in    | Screenshot                       |
|-------------------|---------------|----------------------------------|
| Sidebar IA (6+9)  | e31b522       | `09-devmode-on-sidebar.png`      |
| Brief verb cards  | reason→proactive | `22-brief-contradiction.png`  |
| Composer ∪ ∩ \    | #47 (GRAPH)   | `23-composer.png` / `24-composer-algebra.png` |
| Query + F-1 loop  | #44 + #46     | `25-query-results.png`           |
| Graph view (live) | #47           | `26-graph-live.png`              |
| Ingest pipeline   | always-on     | `27-ingest.png` / `28-ingest-success.png` |
| Settings + usage  | LM-22/23      | `07-settings-usage.png`          |
| Dev toggle        | #46           | `08-settings-devmode-off.png` / `09-…on…` |
| Dashboard         | P5            | `10-dashboard.png`               |
| Threads CRUD      | #47           | `11-threads-empty.png` / `12-thread-created.png` |
| Memory Views      | LM-15         | `13-views.png`                   |
| Context (5-layer) | LM + GRAPH    | `14-context-tracemind.png`       |
| Garden + Louvain  | LM-21         | `15-garden.png`                  |
| Event Graph       | EVG-1/2       | `16-events.png`                  |
| Commitments       | Sprint D F-1  | `17-commitments.png`             |
| Calibration       | P5            | `18-calibration.png`             |
| Traces audit      | always-on     | `19-traces-detail.png`           |
| Inspector (4-tab) | #46           | `21-inspector-brain.png`         |

---

## 1. Sidebar IA — 15 → 6 primary, 9 advanced behind devMode

Before e31b522 the sidebar had 15 items; it now ships with **6 primary
surfaces** (the daily flow: Brief → Composer → Query → Graph → Ingest →
Settings) and **9 advanced surfaces hidden behind Settings → Developer
mode**. Source: `crates/tm-tauri/ui/src/App.tsx:50-83`.

- DevMode **off** (default): `08-settings-devmode-off.png`
- DevMode **on**, sidebar expands: `09-devmode-on-sidebar.png`

The advanced row revealed by the toggle: Dashboard, Threads, Views,
Context, Garden, Events, Commitments, Calibration, Traces, Inspector
— each is a real working surface; we simply do not show them to
newcomers.

## 2. Brief — verb cards (reason → proactive)

`22-brief-contradiction.png` — the Brief is the home screen. Counters
across the top (overdue / open / resolved / candidates / patterns /
insights / proposals / contradictions) feed into cards below. Today
the demo data has **1 contradiction**: cosine -0.99 between
`3235f34b ↔ 4c861106`, surfaced as a single review card.

This is the architecture from `tracemind_reason_to_proactive` —
reason nav was deleted; chains / analogies / contradictions / causal
all surface as **action cards (verbs)** in Brief.

## 3. Composer — composition layer (the wedge)

The Composer is the answer to *"how do you splice context for the
next AI window?"* — graph algebra over threads.

- `23-composer.png` — LEFT thread × OP × RIGHT thread → Compose / Send
  to Window 4 / Save to disk.
- `24-composer-algebra.png` — operator dropdown shows the three
  primitives: **Union (∪)**, **Intersect (∩)**, **Difference (\)**.

Direct evidence the composition algebra from
`tracemind_composition_layer` is shipping end-to-end through the Tauri
commands wired in #47.

## 4. Threads — first-class graph

Sprint GRAPH (#47) made threads a real DB-backed entity, not a UI
fiction.

- `11-threads-empty.png` — empty state, "Each thread is a composable
  slice of memory tied to a session or task."
- `12-thread-created.png` — I created **"Sprint GRAPH walkthrough"**
  via the form. UUID `a8c0193c`, started 10:34 AM, status `open`,
  `End` button visible. The thread immediately becomes selectable in
  the Composer LEFT/RIGHT pickers.

Backend: `crates/tm-graph/src/thread_graph.rs` (Thread CRUD +
materializer), Tauri commands `cmd_threads_list`, `cmd_thread_start`.

## 5. Query — F-1 feedback loop + proactive context

`25-query-results.png` — query *"What is TraceMind?"*:

- Strategy **vector-only** (UCB1 arm 0), Latency **40ms**, 5 entities,
  11 triples
- **"Was this helpful?"** thumbs up / down at the top (Sprint D F-1)
- Per-entity 👍 / 👎 / **wrong ctx** buttons (Sprint D F-1, per-row)
- **"splice"** button = LM-15 Memory Views save-as-splice
- "Why these results? (5 evidence paths)" expandable provenance
- Entities returned with type tag (Person / Concept / Organization) +
  confidence (90%, 70%, …)

This is the full retrieval surface with the F-1 negative-feedback
loop ("wrong ctx" maps to the deny-list signal feeding LM-11).

## 6. Graph — live knowledge graph

`26-graph-live.png` — **394 nodes, 832 edges** (407 - 13 isolated
filtered at min-deg ≥ 1). Force-directed render, nodes colored by
entity type:

```
Person · Organization · Technology · Concept · File · Url · Project · Decision · Event
```

The chips above the canvas are **Louvain community labels** rendered
via c-TF-IDF (LM-21): "TraceMind · Claude · Aaditya", "full ·
retrieval · weights", "capture · pbpaste · zsh_history", "pipeline ·
Title Case · two-pass", "Privacy · design · telemetry", etc. Each
community is a clickable chip that recenters the view.

Controls: All Types filter / min-deg slider / Communities toggle /
Labels toggle / Fit.

## 7. Ingest — typed extraction + dedup gate

`27-ingest.png` → `28-ingest-success.png`:

I ingested *"I'll wrap up the Sprint GRAPH walkthrough doc by tomorrow
and send the rebuilt Tauri bundle to the investor demo deck."*

Result panel:
- ✅ Ingested successfully, trace `c6fc2988`
- ENTITIES EXTRACTED (2): `[Person] I'll`, `[Concept] demo`
- TYPED RELATIONSHIPS: `I'll  Produces  demo`
- + 0 co-occurrence triples

Note the dedup gate at work — the Traces panel (next section) shows
3 prior ingests today were `[SKIPPED: near-duplicate of existing m…]`
by the governance layer.

**Bug surfaced:** the soft-promise "I'll … by tomorrow" did not
populate the Commitments panel. Either the commitment-detection
regex is stricter than the description suggests, or the materializer
hasn't refreshed. Logged for follow-up.

## 8. Settings — capture sources + DP check-in

- `06-settings-top.png` — capture-source toggles. Default install:
  Clipboard / Shell / Notes are **ON BY DEFAULT** (the three labelled
  badges). Screenshot / Browser / Audio / Calendar are **off**.
- `07-settings-usage.png` — **"Your usage (local only)"** panel
  (LM-22/23 DP check-in). Today: 1 query, 0 helpful, 0 not-related,
  first seen 5/15/2026, last query 11m ago, active days 1. **"Copy
  usage JSON (for sharing with your DP contact)"** button — explicit
  opt-in to manually share; nothing leaves the device automatically.

Settings continues past devMode toggle into **Ontology proposals**
(ONT-2 surface — currently empty since the proposer hasn't run on
this dataset), **Schema (power users)** (folded), and **Privacy
invariants** — the audit-list the README invariants section
mirrors.

## 9. Dashboard

`10-dashboard.png` — toplines + retrieval-strategy bandit summary:

| Metric            | Value |
|-------------------|-------|
| Entities          | 407   |
| Triples           | 832   |
| Traces            | 197   |
| Queries (bandit pulls) | 0 |

Five-arm UCB1 bandit table (vector-only / graph-heavy / hybrid /
episodic / colbert), all currently 0 pulls (this demo data hasn't run
queries through the bandit yet). Entity-growth 7-day chart and DRIFT
panel ("entities pulling away from your topic clusters — review or
tag").

## 10. Context — 5-layer entity view

`14-context-tracemind.png` — typed `TraceMind` and pressed *load*.
The view collapses the whole stack onto one entity:

1. **header** — TraceMind, type Organization, domain
   `organization.real`, confidence 1.00, fingerprint `f0014472`,
   created 2026-04-07, updated 2026-05-13
2. **temporal decay** — recency 0% (half-life ~14h since last touch),
   novelty 100%, value (feedback) 0%, frequency 100%
3. **community (143)** — c-TF-IDF labels `$124.20 · $36.00 · $462.05
   #1` plus tag `AI infrastructure` *(side note: a few cluster labels
   pick up raw dollar amounts as top terms — a c-TF-IDF tuning bug
   to investigate)*
4. **outgoing relations (53)** — `RelatedTo local-only 0.40`, `uses
   Rust 0.75`, `RelatedTo memory 0.40`, …
5. **export .md button** — LM-16 markdown export (Obsidian-style PKM)

## 11. Garden — Louvain communities with c-TF-IDF labels

`15-garden.png` — **21 communities** auto-named from top terms
(LM-21):

| #  | Label                                      | Entities |
|----|--------------------------------------------|----------|
| c1 | memory systems · memory ingestion · knowle…| 90       |
| c2 | I'm · I · LoCoMo                           | 76       |
| c0 | BGE-small · MCP packaging · MCP server     | 65       |
| c3 | related concepts · UCB1 bandit algorithm…  | 41       |
| c18| Core Idea · GDPO Instead · ChatGPT         | 15       |
| c17| Gemini · conversation · said               | 14       |
| c5 | Title Case · pipeline · two-pass           | 14       |
| c8 | graph · Rust · ONNX                        | 12       |
| c4 | capture · daemon · monitors                | 12       |
| c12| Aaditya · knowledge · graph                | 12       |
| c7 | Aaditya · knowledge · graph                | 12       |
| c6 | TraceMind · local-only · memory            | 9        |

Each row has a **"view in graph →"** button that jumps to that
community in the Graph view.

Empty `clusters (0)` and `unsorted (0)` panels above — HDBSCAN
hasn't been run on this dataset yet.

## 12. Events — Glean-style action graph (EVG-1/2)

`16-events.png` — *"Edges are only shown once their support count
clears the frequency floor — preventing tangle."*

Five event kinds, color-coded:
```
● capture   ● query   ● commitment   ● outcome   ● decision
```

Buttons: **Refresh** / **Promote sequences** — promote-sequences
takes high-support precedes-edges and writes them as `Procedure`
rows in `tm-episodic`. Empty state today; populates as the user
actually uses the system.

## 13. Commitments — Sprint D F-1 + Project 2026 wedge

`17-commitments.png` — *"Every 'I'll …' / 'by Friday' / 'let me know
when' TraceMind has noticed. Color-coded by state."* This is the
**Commitment primitive** from `tracemind_strategic_direction` — the
wedge.

Empty today (despite my "I'll ... by tomorrow" ingest — see §7 bug
note).

## 14. Calibration — world-model health

`18-calibration.png` — three-quadrant accuracy readout:

| Metric             | Value |
|--------------------|-------|
| Predictions tracked| 0     |
| Resolution rate    | 0%    |
| Contradictions     | 1     |
| Total entities     | 407   |
| Total triples      | 832   |
| Trace events       | 197   |

Plus **Bandit arms (LinUCB)** table (5 arms × {pulls, avg reward}).
Brier score + per-quarter accuracy charts gated for the world-model
v1 (Q-7, Q4 2026).

## 15. Traces — immutable audit trail

`19-traces-detail.png` — clicking the Retrieve trace for "What is
TraceMind?" opens the detail panel:

```
Retrieve  bc47da48-2887-479a-86ed-9d72e1ade1ba
Created   2026-05-15 10:21:18
Entities  5
Triples   22
Strategy  vector-only (arm 0)
Latency   46ms
Raw text  What is TraceMind?

Full provenance chain: every entity and triple extracted from this
event is tracked by ID for complete auditability. No data leaves
your device.
```

Today's left rail shows 3 Ingest entries `[SKIPPED: near-duplicate of
existing m…]` — proof the governance dedup gate is firing.

## 16. Inspector — 4-tab debug surface (devMode only)

`21-inspector-brain.png` — **Brain snapshot** tab, the live-from-disk
brain readout (no caching, samples re-pulled on demand):

| Layer     | Detail                                                    |
|-----------|-----------------------------------------------------------|
| Graph     | 407 entities · 832 triples · 21 Louvain communities · 1 contradiction · 36 pending relations |
| Vector    | BGE-small (cosine) · dim 384 · 407 entities with vector · **100.0% coverage** |
| Bandit    | 5-arm UCB1 + LinUCB (α=0.5000) — narrow / medium / wide / deep / colbert |
| Episodic  | 197 total traces · ring 0/100 · Ingest 193 / Retrieve 4   |
| Governance| PII regex + confidence gate — 14 captures today, 0 blocks |
| Intent    | 0 open commitments · 0 overdue · 3 pending candidates · 0 active silences |

Tabs: **Brain snapshot** · Why this answer · Entity context · Layer
browser. Header reads "Cross-section the brain. Every number is
computed live from `~/.tracemind/`; nothing here is cached or
precomputed."

---

## What's *not* working / open follow-ups

1. **Commitment detection didn't fire** on "I'll wrap up … by
   tomorrow". Either the regex needs widening or the materializer
   hasn't refreshed Commitments panel.
2. **c-TF-IDF dollar-amount labels** — a few community labels in
   `14-context-tracemind.png` are raw $XX.XX strings, meaning the
   tokenizer isn't filtering currency tokens before TF-IDF.
3. **Ontology proposals empty** — `08-settings-devmode-off.png`
   shows the ONT-2 proposer panel exists but has no proposals on
   this dataset. Need to seed cluster signatures or re-run
   `tm-reflect` against the demo data.
4. **NER quirk** — Ingest extracted `[Person] I'll` and `[Concept]
   demo` from the test sentence (heuristic NER fallback). Worth
   gating with the GLiNER ONNX path for production text.
5. **Brain-snapshot bandit pulls = 0** across all five arms in the
   demo data — bandit state in `bandit.json` was reset when the data
   dir moved to `/tmp/tm-demo`.

## Reproduction

```bash
# Rebuild Tauri bundle (no llama-cpp default features needed)
cd crates/tm-tauri/ui && bun run build
cd ../../.. && cargo build -p tm-tauri --release --no-default-features --features custom-protocol
cp target/release/tracemind-app target/release/TraceMind.app/Contents/MacOS/TraceMind

# Launch against an isolated data dir
TM_DATA_DIR=/tmp/tm-demo \
  ./target/release/TraceMind.app/Contents/MacOS/TraceMind
```

Open Settings → Developer mode → toggle on to reveal the 9 advanced
surfaces.
