# Holistic Review — Implementation Status

**Date:** 2026-07-22
**Implements:** `docs/HOLISTIC-REVIEW-2026-07.md`

Every actionable item from the holistic review, with what shipped. Six
commits, 604 tests green across the touched crates, both benchmark surfaces
verified.

---

## The spine (review §6a + §5 P0.1–P0.2)

**Subtract the tool surface to a core 6.** `tools/list` now advertises only
`memory_store`, `memory_query`, `memory_feedback`, `memory_contradict`,
`memory_compose`, `memory_forget` — the verbs that carry the whole loop. The
other 45 tools stay fully callable by name; they are hidden unless
`TM_MCP_ADVANCED=1`. Fewer advertised tools → the host picks the right one
more often, now measurable.

**Measure the MCP server — the product's actual surface.** New `tm-bench-mcp`
crate drives the *real* `tm-mcp` binary over stdio JSON-RPC and scores
tool-selection accuracy (a model-free IDF selector as a proxy for the host
LLM), server latency percentiles, and contract validity. First time the
product was benchmarked as what it ships as.

**Sharpen the descriptions.** The core-6 descriptions led with internal
sprint codes ("Q4.5 —", "Sprint GRAPH —") containing none of the verbs users
say. Rewritten to lead with intent verbs. Measured against the real server:

| | before | after |
|---|---:|---:|
| selection accuracy | 55% | **80%** |
| selection F1 | 0.62 | **0.80** |
| latency p50 / p95 | — | **21ms / 51ms** (first measured) |

**GEPA over descriptions.** `tm-bench-mcp --optimize-on` runs the reflective
loop with a held-out gate (proposals from one split half, edits kept only if
they improve the other) — the same anchor-set discipline `tm-gepa` uses for
retrieval. Honest finding: on a 27-example hand fixture the automated
token-harvest can't beat the hand-authored descriptions (the trigger
vocabulary is too diverse for that little data); the loop is built and tested
for when real design-partner utterances arrive.

**Wire it to the app.** `tm-mcp` loads `~/.tracemind/mcp-descriptions.json`
(the description analog of `policy.json`) and applies overrides at
`tools/list`, so tuned descriptions reach the running host.

---

## The differentiators (review §5 P1.4–P1.5)

**Retraction beat — shipped and measured.** It was only *promised* in the MCP
instructions: `memory_store` never detected contradictions and
`memory_contradict` needed UUIDs a host can't supply. Now
`memory_store` auto-detects, on every store, when a new fact reverses an
existing one on a *functional* predicate (works_at, lives_in, renamed_to,
…) — and surfaces "You told me Carol works at Stripe, but now it's Datadog."
Only functional predicates fire, so set-membership facts never cry wolf.
Every firing is logged to `retractions.jsonl` so the wedge's real trigger
rate is countable.

**Real temporal supersession.** The beat detected reversals but left the old
fact open in the bitemporal store, so an as-of query returned both values as
if simultaneously true. Now `IngestPipeline` closes the superseded fact's
`valid_to` at store time (`TemporalStore::close_validity` →
`GraphStore::supersede_triple`). An as-of query *before* a reversal returns
the old value; *after* it, the old fact is no longer valid. This is what
makes the beat trustworthy rather than lucky — and it is where the
fact-supersession negative result (MVP-STATUS §4) pointed: validity
intervals, not a scoring heuristic.

---

## Scale and cleanup (review §5 P2.6–P2.7, §3)

**ANN — the silent recall cliff is gone.** Signal search loaded the oldest
2,000 embeddings (`ORDER BY id ASC LIMIT 2000`), silently dropping newer
memories past that — recall decaying with install age. Replaced with a
two-stage index: a stored 64-bit SimHash signature per signal (8 bytes) ranks
the whole corpus by Hamming distance (~1 ns each, no cap), then exact cosine
runs only on the top candidates unioned with a recency window so a new memory
is never dropped for a stale signature. LoCoMo scores are byte-identical
(quality preserved, cliff removed).

**Orphans.** Deleted `tm-modal`, `tm-engram` (zero dependents, zero refs) and
`tm-bench-wme` (zero tests). Wired the previously-orphaned
`tm-graph::contradiction_rate` into the nightly run. (`tm-cluster`, `tm-pgm`,
`tm-temporal` were found to be actually used and kept.)

**Growth engine.** `NightlyScheduler::run()` returned hardcoded `true`s with
no work behind them — the review's core anti-pattern. It now reports only
real signals: retraction firings from the log, and the contradiction rate
computed from the local graph. `tracemind nightly` prints them.

---

## Items not implemented, with reasons

- **§5 P0.3 — five instrumented design partners.** Cannot recruit humans. The
  *instrumentation* they would need is done: `memory_feedback` is wired to the
  bandit (prior session), the retraction beat logs every firing, and
  `tracemind nightly` reports W2-style signals. Recruiting is the founder's
  job; the measurement is ready.
- **§5 P2.8 — real LoCoMo / LongMemEval data.** Requires downloading external
  datasets; not reliably doable in this environment. The train/test harness is
  in place to consume them the moment they're present.
- **§5 P3.9 — decompose `query()` into declared phases.** A multi-day refactor
  of the ~1,000-line hot path that produces the tuned 70.49 F1. Doing it
  hastily risks regressing the carefully-verified retrieval for limited
  immediate user value (the review ranks it P3, lowest). Deferred deliberately
  rather than shipped as risky scaffolding — which would be the very
  "capability outran validation" pattern this whole effort corrected.
- **§6b — the on-device self-improvement loop as a standalone product.** A 2027
  strategic bet, gated on P0 evidence, not a now-implementation.

---

## Test + benchmark state

- **604 tests green**, 0 failures, across every touched crate.
- **LoCoMo (retrieval):** F1 70.49 held-out / 50.32 train — unchanged by all
  of the above (quality preserved).
- **MCP surface:** 80% tool selection, 0.80 F1, p95 51ms, 4/4 contract — the
  surface that was never measured, now measured and improved.

The through-line held: every change bought *evidence that a real interaction
got better*, not more unmeasured capability.
