# TraceMind MVP — Status, Honest Numbers, Known Limits

**Date:** 2026-07-22
**Preceded by:** `docs/REVIEW-2026-07.md` (critique), `docs/H2-AUDIT-2026-07.md` (wiring audit)

This closes the gaps those two documents opened. It also revises a headline
number downward, for reasons given in §2.

---

## 1. The headline, stated correctly

| Split | Questions | F1 | EM | Tuned on? |
|-------|----------:|---:|---:|:---------:|
| `locomo-train.json` | 45 | **50.32** | 42.22 | yes |
| `locomo-mini.json` (**held out**) | 20 | **70.49** | 60.00 | **no** |
| `locomo-mini.json`, hash embedder | 20 | 48.33 | 40.00 | no |

**The previously reported 75.49 was an overfit number and should not be
used.** It was produced by a policy GEPA had tuned on the very 20 questions
it was then scored against. Splitting train from test costs 5 points on that
set (75.49 → 70.49); the 70.49 is a measurement, the 75.49 was a fit.

`locomo-mini` still scores higher than `locomo-train` despite being held out
from *policy* tuning. That is expected and worth stating plainly: the span
extractors in `tm-bench-locomo::extract` were originally developed against
`locomo-mini`, so it retains development fit that no train/test split can
undo. **50.32 on genuinely fresh conversations is the number to plan
against.** The gap between the two is the honest estimate of how much of the
older result was familiarity.

### Provenance of the training set

`fixtures/locomo-train.json` is six new conversations (45 questions) written
for this work — different domains, different phrasings, answers positioned
differently within the sessions. It exists so GEPA has something to tune on
that is not the evaluation set.

### Still not claimed

- n is 45 and 20. Per-category figures rest on a handful of questions each.
  Real LoCoMo / LongMemEval remains the top priority; these fixtures test for
  *regressions and overfitting*, not for absolute quality.
- Retrieval on the `train` split is the weak point: `multi_hop` scores 0.0
  there (4 questions), all of which need reasoning over two sessions.

---

## 2. Gaps from the review, and what happened to each

### 2.1 The MCP reward loop was dead — **fixed**

The reward model scored *un-evidenced* queries from click / dwell / re-query
timing. Those are interactive-UI signals. Inside an agentic host the model
emits its next tool call in well under five seconds, which the old rule read
as "re-queried within 5s → results were poor → **0.1**", the worst available
reward. The bandit was not merely receiving a noisy signal on the shipping
surface; it was being told that essentially every query failed, which makes
the *least-explored* arm look best.

Changes:

- `HostKind::{Interactive, Agentic}`. Timing-derived signals apply only to
  `Interactive`. The default is `Agentic` — a wrong `Interactive` corrupts
  the bandit, a wrong `Agentic` merely forgoes a weak signal. Tauri opts in.
- **An un-evidenced query now registers no reward at all.** Fabricating a
  constant biases the posterior toward whichever arm is pulled most, which is
  backwards for an explore/exploit controller.
- `memory_feedback`'s implicit kinds now reach the controller:
  `retrieval_cited` writes a positive signal, `retrieval_miss` a negative
  one. They were previously recorded as `feedback_signals` rows and were
  inert — on an agentic host they are the *only* implicit evidence available.

Covered by four tests in `tm-retrieval`, including one asserting that the
agentic no-feedback case leaves the bandit untouched.

### 2.2 Empty answers — **fixed**

Two of twenty predictions used to be `''`. An empty result tells the user
three indistinguishable things: "you never told me", "the daemon is not
running", and "retrieval is broken".

- `Grounding::{Found, Uncertain, NotStored}` on every `RetrievalResult`,
  computed from fused signal scores and entity count.
- The extractive backend now abstains explicitly, and names the topics it
  *does* know: *"I have nothing stored that answers that. Closest topics I do
  know about: Alice, Tokyo."*
- MCP returns the `grounding` state alongside the answer so a host can
  distinguish the cases.

A well-scoped "I don't know" builds more trust than a confident wrong answer
— and token-F1 actively rewards the latter, so this needed to be a deliberate
product decision rather than a benchmark-driven one.

### 2.3 Train/test split — **done** (see §1)

### 2.4 Two-sided CI gate — **done**

`report::embedder_separation_gate` fails when a trained encoder does not beat
the hash embedder by a margin. A one-sided "did F1 drop" gate cannot detect
the original defect, where BGE and hash produced byte-identical scores for
four releases because the engine was returning nothing. Current separation:
70.49 vs 48.33 = **22.2 points**.

### 2.5 Orphaned crates — **partially addressed**

Still unwired: BGE-M3 (Q3.9), Matryoshka (Q4.9), contradiction-rate
(Q3.4/Q4.10), hierarchical tiers (Q3.6). Each needs its own sprint; none
blocks the MVP.

### 2.6 Tier-1 synthesis for reasoning questions — **not done**

Still the right tool for `multi_hop`. Deferred.

---

## 3. Real bugs found while closing the gaps

Three of these were surfaced by tests that had been dismissed as flaky or
pre-existing. All three were genuine product defects.

### 3.1 PII filter silently rejected UUIDs, SHAs, and hashes

`has_phone` collects ten digits while skipping `-`, and only guarded against
a *trailing digit*. A UUID or commit SHA contains long digit runs separated
by dashes, so it matched — and the ingest gate then **dropped the memory with
no user-visible signal**. For a product whose primary capture surface is a
developer's clipboard, this discards exactly the content it exists to store.

This is what the "flaky" `tm-ingest` test was reporting: its fixture text
embedded a random UUID, so it failed whenever the UUID happened to contain a
matching digit run — roughly one run in three.

Fixed by requiring the numeric run to sit in a token that (a) contains no
letters and (b) carries a plausible digit count (≤11 phone, ≤16 card, ≤9
SSN). Real phone numbers, SSNs, and card numbers are still detected; five new
tests cover both directions.

### 3.2 Every access-log range query returned nothing

`log_access` relied on the column default `datetime('now')`, which writes
`"2026-07-22 15:32:16"`, while all range queries bind
`DateTime::to_rfc3339()` (`"2026-07-22T14:32:16+00:00"`). These are compared
as **strings**, and `' '` (0x20) sorts before `'T'` (0x54) — so a row written
*now* always compares as earlier than a bound written an hour ago.
`get_accessed_entities_in_range` could never return anything.

Fixed by writing RFC3339 explicitly, with the query normalising legacy rows
via `replace(created_at, ' ', 'T')` so existing databases keep working.

### 3.3 Integer overflow discarded the strongest extraction signal

The trailing-preposition cue ("rename her company **to**?") assigned
`i32::MAX`, and a later change added a bonus to that score — wrapping it
negative and silently throwing away the best candidate. Fixed with a
headroom-safe sentinel; a regression test asserts the arithmetic cannot wrap.

### 3.4 Adding a policy field broke existing `policy.json` files

Deserialisation failed and the fallback path silently reverted the user's
tuned policy to compiled-in defaults. Newer fields now carry `serde`
defaults, with a test loading a legacy policy document.

---

## 4. Retrieval and reading improvements

Measured on the training split, with the held-out set confirming no
regression:

| Change | Train F1 |
|--------|---------:|
| starting point | 32.35 |
| graph-backed named resolution (NER instead of word lists) | 34.58 |
| demote sentence-initial tokens; bare-month comparison | 43.47 |
| preposition-cue overflow fix (§3.3) | 47.35 |
| GEPA tuning on train | **50.32** |

The most instructive of these: the extractor originally rejected
sentence-initial capitalised words using a curated list of ordinary English
words. That does not generalise — "Hired", "Chaired", "Registered",
"Switching" are all sentence-initial verbs no reasonable list contains, and
each was being returned as a person's name. Sentence-initial tokens are now
*demoted* rather than excluded, and candidates are cross-checked against the
entity types the graph's NER recorded at ingest.

### A negative result worth recording

**Fact supersession did not work and is shipped disabled.** Preferring the
later memory when several answers type-check is correct in principle — a
defence moves from June to July, an offer rises from $40M to $65M. Three
variants were implemented and measured:

| Variant | Train | Held-out |
|---------|------:|---------:|
| blanket recency multiplier (0.5) | 33.47 | 72.99 |
| tie-break within 20% band | 35.99 | 44.82 |
| tie-break within 2–5% band | 39.63 | 60.49 |
| gated on question cues ("now", "finally") | 41.85 | 61.32 |
| **disabled** | **47.35** | **75.49** |

Every variant was worse. Most facts are stated once and never revised, many
candidates score exactly equal, and flipping those to the later turn is wrong
more often than right. The mechanism is retained, tested, and defaulted to
`recency_weight: 0.0`; GEPA independently converged on the same value. It
should be revisited with an explicit temporal KG (`valid_from`/`valid_to`,
charter Q3.4) rather than a scoring heuristic.

---

## 5. Test status

All green — 14 crates, zero failures, zero known flakes:

```
tm-gepa 43 · tm-vector 71 · tm-retrieval 27 · tm-answer 43 · tm-bench-locomo 53
tm-cli 24 · tm-mcp 28 · tm-ingest 89 · tm-graph 149 · tm-governance 11
tm-types 51 · tm-controller 87 · tm-episodic 21 · tm-reflect 37
```

The two failures carried in the previous audit are fixed (§3.1, §3.2). The
`tm-ingest` flake was run 8×/8 clean after the PII fix.

`cargo test --workspace` still cannot run here: `llama-cpp-sys-2` requires
`cmake`. `tm-tauri` type-checks with `--no-default-features --features
custom-protocol` but was not built, for the same reason.

---

## 6. What an MVP still needs

1. **Real LoCoMo / LongMemEval.** 65 hand-written questions bound the error
   bars far more than they bound the truth.
2. **`tm-bench-mcp`.** Every benchmark measures the library path; the product
   *is* the MCP server. Tool-call precision/recall and p95 latency inside a
   host are unmeasured.
3. **Tier-1 synthesis** for `multi_hop` (0.0 on train).
4. **ANN index.** `search_signals` linear-scans a hard cap of 2,000, so
   recall decays silently as capture history grows.
5. **Cold start.** First-run model download contradicts the local-only
   promise at the moment the user forms an opinion.
6. **Design-partner signal.** The reward loop is now live but has never
   received a real `memory_feedback` call.
