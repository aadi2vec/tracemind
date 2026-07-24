# TraceMind — Holistic Review, Post-H2

**Date:** 2026-07-22
**Preceded by:** `REVIEW-2026-07.md` (critique) · `H2-AUDIT-2026-07.md` (wiring) · `MVP-STATUS-2026-07.md` (gap closure)
**This document:** steps back from the code to assess the *product*, then proposes where to take it next.

---

## 0. TL;DR

The engineering is genuinely strong and the wedge is genuinely differentiated.
The problem is not capability — it is that **an enormous amount of capability has
been built against a mechanism no real user has ever exercised.**

Three numbers frame everything:

- **87,000** lines of Rust across 32 crates.
- **51** MCP tools exposed to hosts.
- **0** design partners with real sessions. (The H2 charter's own Q4 exit gate
  required ≥3 with ≥15 sessions each. There is no evidence in the repo it was met.)

The last three work sessions moved the *evidence* from "unmeasured" to "honestly
measured" — retrieval now genuinely works (50.32 F1 on fresh conversations, and
the trained embedder beats the hash by 22 points where it used to tie). That was
the right work. But it also revealed the shape of the risk: TraceMind is being
built as if the open question is "what more can it do," when the actual open
question is **"will anyone leave it installed after a week."**

The single most valuable thing to do next is not another capability. It is to put
the one differentiated behavior — the *retraction beat* — in front of five real
people and instrument whether it lands.

---

## 1. Honest scorecard

| Dimension | State | Evidence |
|-----------|-------|----------|
| **Retrieval quality** | Real, modest | 50.32 F1 fresh / 70.49 held-out; BGE beats hash 70.49 vs 48.33 |
| **Self-improvement (GEPA)** | Works, wired, offline-only | +2.96 F1 on train from executed rollouts; `policy.json` applied at engine open |
| **Reward loop** | Live, evidence-gated, untested by a human | 4 tests; **0 real `memory_feedback` calls ever** |
| **Local-only privacy** | Real, not marketing | no network in request path; PII gate now doesn't drop UUIDs |
| **Answer quality (abstention)** | Correct behavior shipped | never returns empty; names nearby topics |
| **MCP surface** | Vast, unmeasured | 51 tools; **0 measured for tool-call precision/recall** |
| **Scale** | Cliff at ~2k signals | `search_signals` linear-scans a hard cap of 2000; no ANN |
| **Desktop app** | 7.5k LOC, 3 tests, P5 | correctly demoted; not the front door |
| **Distribution** | Thesis clear, unproven | "MCP into Claude Code"; no install → retention data |

**What is genuinely good, stated plainly** so the critique is calibrated:

- `tm-graph` (14k LOC, 149 tests) is a serious knowledge-graph engine.
- The trace log is a real asset — an immutable, content-addressed audit trail is
  exactly the substrate GEPA needs, and almost nobody in this space has it.
- The Tier 0/1/2 answer dispatcher with Tier-0 always succeeding is the right
  invariant for a local product.
- ~900 tests at this LOC, now with zero known failures or flakes, is above
  average for a solo founder.
- The three "flaky/pre-existing" test failures turned out to be **real product
  bugs** (PII dropping developer content, every access-log query returning
  nothing) — the test suite was doing its job; it just wasn't being believed.

---

## 2. The central tension: capability outran validation

This is the through-line of all four review documents, and it is worth naming
directly rather than as a list of fixes.

**Symptom 1 — 51 MCP tools.** `memory_query`, `memory_store`, `memory_reason`,
`memory_analogies`, `memory_compose_{union,intersect,filter}`, `memory_contradict`,
`memory_pin`, `memory_forget`, `memory_arc`, `memory_sentiment`,
`memory_outcome_proposals`, `memory_pattern_silence`… Each is real code with
tests. But a host model has to *choose* to call each one at the right moment, and
**not one of those 51 tool descriptions has been measured for whether the model
actually calls it correctly.** The product's real prompt — the only text
TraceMind controls inside the host's context — is those 51 descriptions, and it
is unoptimized and untested. Fifty-one tools is not a feature set; it is fifty-one
chances for the host to call the wrong thing.

**Symptom 2 — orphaned crates.** `tm-vector::bge_m3`, `matryoshka`,
`tm-graph::contradiction_rate`, `tm-pgm`, `tm-temporal`, `tm-cluster`,
`tm-modal`, `tm-engram` are all built and (mostly) tested but referenced by no
binary. That is thousands of lines of maintained-but-unreachable code. Each was
built because the charter listed it, not because a user needed it.

**Symptom 3 — the eval that wasn't.** For four releases the headline F1 was
produced by a keyword scanner while the retrieval engine returned nothing, and
nobody noticed because the number was stable. Capability was added on top of a
foundation that was never checked.

The corrective is not "stop building." It is to **re-anchor the definition of
done from 'the charter item is implemented' to 'a real interaction got
measurably better.'** The GEPA loop is the proof this is possible: it only became
meaningful once it executed against real evaluations instead of self-reported
scores. Every part of the product needs that same shift.

---

## 3. Architecture: what is load-bearing vs. dead weight

Ranked by dependents and binary-reachability:

**Load-bearing core (keep, invest):**
`tm-types` (26 deps) · `tm-graph` (10) · `tm-ingest` (8) · `tm-intent` (7) ·
`tm-retrieval` (6) · `tm-episodic` (6) · `tm-vector` (4, now with BM25) ·
`tm-controller` (4) · `tm-answer` (4) · `tm-gepa` (3, now wired).

**Reachable but thin / questionable ROI:**
`tm-reflect` (5.5k LOC, 87 tests — large surface for uncertain user value) ·
`tm-world-model` (2.8k LOC — the `f_outcome` predictor; extension target, but
does anyone see its output?) · `tm-reason` (chains/analogies — surfaced via MCP
but unmeasured for usefulness).

**Orphaned — decide: wire or delete:**
`tm-pgm`, `tm-temporal`, `tm-cluster`, `tm-modal`, `tm-engram`,
`tm-vector::bge_m3`, `tm-vector::matryoshka`, `tm-graph::contradiction_rate`.
Recommendation: **delete `tm-modal` and `tm-engram`** (350–548 LOC, no path to
use); **fold `tm-temporal` into the contradiction/supersession work** when a real
temporal KG is built (see §5); **keep `bge_m3`/`matryoshka` dormant** only if
BGE-M3 is on the near roadmap, otherwise delete.

**Benchmark sprawl:** 8 `tm-bench-*` crates (`bench`, `context`, `locomo`,
`longmem`, `memory`, `triples`, `wme`), 3 with zero tests and zero use. Collapse
to two: `tm-bench-locomo` (retrieval quality, now with train/test split) and a
new `tm-bench-mcp` (see §5). Delete the rest or archive them.

**The `query()` method is still ~1,000 lines.** The `Space`/`ComposedIndex`
refactor started the decomposition; finishing it (each phase an object with
declared inputs/outputs) is the precondition for GEPA to optimize *structure*,
not just weights.

---

## 4. Is the wedge still right?

Yes — and it is the strongest thing about the company. The wedge has three layers,
all still valid:

1. **Local-only ambient memory for every AI you use** — the privacy story is real
   and now technically honest.
2. **The retraction beat** — the moment the agent says *"you told me last week you
   weren't doing X anymore."* No other memory layer ships contradiction-over-time
   as a first-class behavior. This is the demo.
3. **Composition** — the graph algebra (`union`/`intersect`/`filter`/`bridge`)
   that lets a user compose context from past conversations into the next one.
   This is the moat, and it is the least proven of the three.

The distribution thesis — **ship as an MCP server into Claude Code first** — is
correct and under-exploited. Install is a 4-line JSON edit; every developer who
tries it and finds it useful leaves it installed. But that "finds it useful" has
never been observed.

**The risk to the wedge is not competition — it is that the two differentiating
layers (retraction, composition) are the two that have never been validated with
a user, while the effort has gone into breadth (51 tools) that any memory layer
could match.**

---

## 5. Next steps — prioritized

Ordered by *evidence gained per unit effort*, not by capability.

### P0 — Prove the loop with humans (weeks, not months)

1. **`tm-bench-mcp`.** A scripted host transcript replayed against the real
   JSON-RPC server, scoring tool-call precision/recall and p95 latency. The
   product *is* the MCP server; it has never been measured as one. This is the
   single highest-leverage missing artifact.
2. **GEPA over the tool descriptions.** Once `tm-bench-mcp` exists, the 51 tool
   descriptions become an optimizable prompt with a real scorer — the same
   machine that tuned retrieval weights, pointed at the actual product surface.
   Start by *cutting* the surface: measure which tools the host ever calls, and
   demote the rest behind a `verbose` capability.
3. **Five design partners, instrumented.** Ship the Claude Code integration to
   five real developers with `memory_feedback` wired to fire automatically on
   citation/miss (already built — §2.1 of MVP-STATUS). The metric is not F1; it
   is **W2 retention** and **whether the retraction beat ever fires in a real
   session.** The H2 charter already named this gate; it was never run.

### P1 — Make the differentiators real

4. **Retraction beat as a shipped, measured behavior.** Contradiction detection
   exists (`tm-graph`, `tm-reason`) but is surfaced passively. Make it a
   proactive card the host can raise, and instrument how often it fires and
   whether the user acts on it. This is the wedge; it should be the most
   measured thing in the product, and it is currently among the least.
5. **Real temporal KG (`valid_from`/`valid_to`).** The negative result on fact
   supersession (four variants, all worse — see MVP-STATUS §4) is the signal:
   recency-as-a-scoring-heuristic is the wrong tool. Supersession needs explicit
   validity intervals, which is also what makes the retraction beat correct
   rather than lucky. This is where `tm-temporal` earns its place.

### P2 — Remove the scale cliff and the dead weight

6. **ANN index for signals.** `search_signals` linear-scans a hard cap of 2,000;
   at 100k captures from the daemon, recall degrades *silently as a function of
   how long the user has had it installed* — the worst possible failure shape.
   Fine today, P1 before any user has a month of history.
7. **Delete or wire the orphans (§3).** Every orphaned crate is maintenance cost
   with no user on the other end.
8. **Real LoCoMo / LongMemEval data.** 65 hand-written questions bound
   regressions, not truth. Get the published sets before quoting any number
   externally.

### P3 — Finish the structural refactor

9. **Decompose `query()` into declared phases**, so GEPA can search structure
   (which phases run, in what order) and not only scalars.

---

## 6. The next product

Two readings of "next product." Both matter.

### 6a. The next *version* of TraceMind: subtract, then sharpen

The honest move post-H2 is **radical subtraction**. The product has out-built its
evidence. The next version should expose **one loop a user can feel**, not 51
tools:

> *Your agent remembers what you told it, notices when you contradict yourself,
> and carries the right context into your next conversation — locally, on your
> machine, with no cloud.*

Concretely:
- **Collapse the MCP surface** from 51 tools to a **core 6**: `memory_store`,
  `memory_query`, `memory_feedback`, `memory_contradict` (the beat),
  `memory_compose` (the moat), `memory_forget` (the trust primitive). Everything
  else moves behind an opt-in `advanced` capability. Fewer tools = the host calls
  the right one more often, which is directly measurable.
- **Make the retraction beat the product's identity.** It is the one sentence a
  design partner will repeat to another developer. Build the entire onboarding
  around producing that moment in the first session.
- **Ship the feedback loop as the growth engine.** The reward loop is live but
  starving. Wire implicit signals aggressively, and let the nightly GEPA run
  personalize the policy per-user from *their* trace log. "It gets better the more
  you use it, and never leaves your machine" is a story no cloud memory can tell.

### 6b. The genuinely next product: personalized on-device policy as a primitive

The deepest thing built in the last three sessions is not the retrieval score. It
is the **loop**: trace log → executed evaluation → verified policy update →
applied at runtime, entirely on-device. That loop is more novel than the memory
product wrapped around it.

The next product is **that loop as infrastructure**: a local, private,
self-improving policy layer that any on-device agent can adopt — not just
TraceMind's retriever. The pitch is "your agent's behavior tunes itself to you,
overnight, on your hardware, and the training data never leaves." Memory is the
first application of it; agent-harness optimization, tool-selection policies, and
personalized routing are the next. This is the `tm-gepa` + trace-log + `policy.json`
stack, generalized past retrieval.

That is a 2027 bet, gated on the same thing everything else is: **evidence, from
real users, that the loop makes their experience measurably better.** Which is why
P0 is P0.

---

## 7. The one-sentence version

TraceMind has built a remarkable amount of correct, well-tested machinery around a
genuinely differentiated wedge — and the next unit of effort should buy *evidence
that a human wants it*, not more machinery, because the machinery is already well
ahead of the proof.
