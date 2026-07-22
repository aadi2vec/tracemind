# AIE World's Fair 2026 → TraceMind Backlog

**Purpose:** Capture everything discussed in the six-week research window that fed into the H2 2026 pivot, so the ideas that **did not make the charter** are not lost. This is a companion to `docs/CHARTER-H2-2026.md`, not a replacement.

**Effective:** 2026-07-22
**Owner:** Aaditya Srivathsan
**Sources:**
- AIE World's Fair 2026 keynotes + track talks (Anthropic / DeepMind / Amazon / independent research)
- NirDiamant `Agent_Memory_Techniques` 30-technique catalog
- Engram Labs public launch materials
- GEPA paper + Superlinked Spaces docs + BGE-M3 model card
- Related workbooks under `AIE_WorldsFair_2026/` (`TraceMind_Gap_Analysis_vs_AIE2026.docx`, `TraceMind_Novel_Ideas_Backlog.xlsx`, `AIE_2026_Question_Playbook.docx`)

---

## 1. Framing

Three themes dominated AIE 2026 keynotes:

1. **Recursive self-improvement** — systems that improve their own improvement loop, not just their outputs.
2. **Memory as continual learning** — the substrate you retrieve from *is* the update, no gradient step required.
3. **Context engineering as the primary craft** — retrieval quality is doing more of the work than model size.

The H2 charter picked the highest-leverage instantiations for TraceMind. This document tracks the rest.

---

## 2. Ideas Adopted into H2 Charter (cross-reference)

Included here so the delta with §3 is unambiguous.

| Source                                | Where it landed                                       |
|---------------------------------------|-------------------------------------------------------|
| GEPA (Genetic-Pareto prompt evolution)| Pillar 1 + `tm-gepa` crate (Q4.1–3)                    |
| Superlinked Spaces pattern            | Pillar 2 + `ComposedIndex` (Q3.10)                    |
| BGE-M3 (dense + sparse + multi-vector)| Q3.9 dense (scaffolded), Q4 multi-vector              |
| Matryoshka nested embeddings          | Q4.9 (4 tiers 128/256/512/768d)                       |
| Reflexion (verbal post-mortem)        | Q3.8 `SessionReflection`                              |
| ACE Generator / Reflector / Curator   | Q4.11 + Q4.13 recursive Curator                       |
| GVU (Generator-Verifier-Updater)      | Implicit in GEPA + `VerifierGate` (Q4.3)              |
| Memory-as-tools verbs                 | Q4.5 (pin / forget / promote / contradict / reflect)  |
| Memory routing (should-we-retrieve?)  | Q3.2 `MemoryRouter` upstream of LinUCB                |
| Hierarchical hot/warm/cold tiers      | Q3.6 `MemoryTier` + `TierStore`                       |
| Working-memory pins                   | Q4.6 pins in `ComposedIndex`                          |
| LongMemEval + BEAM benchmarks         | Q3.7 `tm-bench-longmem` crate                         |
| Contradiction-rate as first-class axis| Q3.4 + Q4.10                                          |
| Engram Labs competitive positioning   | Charter §3 positioning table                          |
| QLoRA / LoRA on-device personalization| **Deferred to 2027** (GEPA replaces for retrieval)    |

---

## 3. Backlog — Ideas NOT in the H2 Charter

Grouped by theme. Each entry: *what it is*, *why it was cut*, *what would trigger revisit*, *estimated cost if we did it*.

### 3.1 Skill / procedure libraries (Voyager, SAGE line of work)

**What it is.** Voyager-style skill libraries: the agent discovers reusable procedures, stores them as executable code + preconditions + postconditions, and composes them across tasks. SAGE extends this with self-generated curriculum.

**Why cut from H2.** TraceMind stores facts and events; it doesn't execute procedures. Adopting Voyager-style skills would require an execution layer (sandboxed code runner, precondition checker) that has no wedge overlap with the memory-OS thesis. Also: the "learnable procedures" stub in `tm-cli proc list` is already empty and has been for two sprints — evidence that we don't have real demand for this yet.

**Revisit trigger.** If Pillar 3's memory-as-tools verbs (pin/forget/promote/contradict/reflect) show usage patterns that look like "the user is building recipes," treat that as demand signal for a `procedure_capture` verb.

**Estimated cost.** ~2500 LOC + a sandboxed execution surface + safety review. Not H2.

---

### 3.2 HyperAgents / population-based prompt search

**What it is.** Instead of a single GEPA lineage, run a population of N policies in parallel, tournament-select, crossover. Higher exploration, more compute.

**Why cut from H2.** On-device compute budget doesn't tolerate N-way parallel rollouts. GEPA's *sequential* reflective mutation is the whole reason we picked it over GRPO — 35× fewer rollouts. Population-based search would blow the local-only constraint.

**Revisit trigger.** If Pillar 7's loop-efficiency metric plateaus (Q4 exit gate conjunct 2 fails), consider a *small* population (N=3) with weight-sharing to break out of local optima.

**Estimated cost.** ~800 LOC on top of `tm-gepa` + a compute-budget governor. Post-2026.

---

### 3.3 BridgeRAG / BDTR (bridge-conditioned retrieval for multi-hop)

**What it is.** Explicit "bridge" retrieval: given a multi-hop query, first retrieve *intermediate* documents whose only job is to bridge query → answer. Improves multi-hop F1 on LongMemEval.

**Why cut from H2.** ComposedIndex + KG-R1 4-action traversal in `tm-graph` already do bridging *implicitly* via hop expansion (arms 1/2/3). Adding an explicit bridge stage before we've measured how much of LongMemEval-multi-hop is being missed by the implicit path would be premature.

**Revisit trigger.** Q3.7 publishes LongMemEval baseline. If `multi_hop` category F1 lags `single_hop` by > 8 points, add BridgeRAG as a QueryPlanner action route.

**Estimated cost.** ~600 LOC — a `PlanAction::Bridge` variant + a bridge-scoring space in `ComposedIndex`.

---

### 3.4 Slipstream / trajectory-grounded compaction validation

**What it is.** Before compacting/consolidating memories, replay a set of *anchor trajectories* against the pre- and post-compaction store. Reject compactions that degrade replay quality. Same idea as our `VerifierGate` (Q4.3) but applied to consolidation instead of policy updates.

**Why cut from H2.** Consolidation was replaced by a LinUCB `should_compact` arm (Pillar 3, per charter deprecations). The arm gets bandit-corrected by outcome anyway — Slipstream would be belt-and-suspenders.

**Revisit trigger.** If `should_compact` bandit posteriors show high variance (arm can't decide), Slipstream anchor replay is the correct escalation — it turns a stochastic arm into a deterministic gate.

**Estimated cost.** ~500 LOC. Trivial to add once anchor-eval infrastructure from Q4.3 exists.

---

### 3.5 Full NirDiamant 30-technique catalog — remainder

The subset we adopted is listed in §2. The rest, ranked by fit:

| Technique                       | Fit? | Note                                                                 |
|---------------------------------|------|----------------------------------------------------------------------|
| Episodic buffer time-decay      | Med  | Overlaps `RecencySpace` in Q3.10; not additive.                       |
| Salience-weighted encoding      | High | Would fit as a new `SalienceSpace` in ComposedIndex — see §3.6.       |
| Contextual boundary detection   | Med  | Related to `session_scopes` (Q3.5); needs behavioral trigger design.  |
| Semantic clustering on write    | Low  | Cluster-on-read (existing `tm-cluster`) is cheaper.                   |
| Retrieval-augmented generation loop | Low | We're the memory, not the generator; out of wedge.                    |
| Chunk overlap tuning            | Low  | Sub-token; Matryoshka slicing already gives us granularity control.   |
| Memory rehearsal / spaced repetition | High | Would fit as a `rehearse_due()` verb — helps Commitment→Outcome loop. See §3.7. |
| Cross-encoder reranker on top-K | Done | ColBERT MaxSim (arm 4) already covers this.                           |
| Sliding-window summarization    | Low  | Compaction handles this; would duplicate.                             |
| Query rewriting via LLM         | Med  | Would need Tier-1/2 LLM in request path — violates local-first budget.|
| Hypothetical document embeddings (HyDE) | Med | Same latency concern as query rewriting.                             |
| Multi-query fusion              | Med  | Cheap addition on top of `ComposedIndex`. See §3.8.                   |
| Adaptive top-K by query type    | Done | LinUCB arms already do this.                                          |
| Time-aware entity resolution    | High | Fits with Q3.4 temporal KG; would tighten contradiction detection.    |
| Long-context caching            | Low  | We're the cache; recursive.                                           |

**Load-bearing additions worth extracting:** Salience-weighted encoding (§3.6), rehearsal/spaced-repetition (§3.7), multi-query fusion (§3.8), time-aware entity resolution (fold into Q3.4 follow-up).

---

### 3.6 Salience layer as fifth graph layer

**What it is.** From the [[tracemind-layered-graph-guidance]] memory: the investor guidance flagged salience as its **own** layer, distinct from confidence. Confidence = "how sure are we this fact is true"; salience = "how much does the user care about it right now."

**Why cut from H2.** Q3.10 ComposedIndex has `ConfidenceSpace` but no `SalienceSpace`. Salience needs signal fabric (Q3.1) as an input, so it was chicken-and-egg with the merge. Now that Q3.1 shipped, salience is unblocked.

**Revisit trigger.** As soon as Q3.1 feedback signals accumulate for ~2 weeks, salience becomes computable as `f(recency, reuse_rate, verb_affinity_hit_rate)`.

**Estimated cost.** ~400 LOC — new `SalienceSpace` in ComposedIndex + `salience_score()` derivation in `tm-episodic`. Slot into Q3/Q4 hotfix window.

---

### 3.7 Spaced-repetition rehearsal on the Commitment ledger

**What it is.** For open Commitments, schedule a low-cost "rehearsal" retrieval (Anki-style spaced intervals) that surfaces the Commitment in the daily Brief. Users who forget commitments get gently reminded; the reminder itself is a signal about salience.

**Why cut from H2.** Outcome prompt scheduler exists in `tm-reflect` but drives on urgency, not spaced-repetition psychology. Adding SR would have required tuning intervals against a real user cohort — not available in Q3.

**Revisit trigger.** After 3+ active DPs (which is the Q4 exit-gate prerequisite anyway), tune SR intervals against their real-world Commitment slip rates.

**Estimated cost.** ~300 LOC — SR schedule table + Brief integration. Fold into Q4.15 as a stretch item.

---

### 3.8 Multi-query fusion on `ComposedIndex`

**What it is.** For high-value verbs (`contradict`, `plan`), issue 2–3 paraphrased variants of the query, run each through `ComposedIndex`, fuse with RRF. Improves recall on adversarial rewordings without changing the substrate.

**Why cut from H2.** Q3.10 shipped single-query ComposedIndex; multi-query needs a paraphraser. Local paraphrasing requires Tier-1 LLM in request path (Tier-1 currently structured tasks only). Would drift into the "no LLM in request path" rule.

**Revisit trigger.** If per-verb F1 shows > 5 point spread between paraphrased-eval and canonical-eval (Q3.7 LongMemEval discovery), multi-query becomes worth the latency budget.

**Estimated cost.** ~300 LOC + a small paraphrase template library (Tier-0 templated, not LLM). Cheap.

---

### 3.9 Autoresearch loop closure — user-facing "research mode"

**What it is.** Full Generator/Verifier/Updater loop exposed to the user: "run this open question for a week; TraceMind will accumulate evidence, contradict itself, and Brief you when it converges." A user-facing autoresearch primitive.

**Why cut from H2.** The internal L1/L2/L3 improvement loop is the priority; a user-facing autoresearch surface would need product design, safety UX (what happens when the loop finds contradictions with user beliefs?), and a much more careful Verifier. Ship the internal loop first, learn from it.

**Revisit trigger.** After the Q4 exit gate (Δ F1 ≥ +3 with no code deploy) fires, the internal loop is validated. Then expose a `question_pin` verb that turns any Brief item into an ongoing research target.

**Estimated cost.** ~1500 LOC + product design pass. 2027 H1.

---

### 3.10 Engram Labs replication — full LoRA sidecar path

**What it is.** The 2026 Engram launch pattern: cloud personalization API that fine-tunes a small LoRA adapter on each user's memory nightly, serves personalized generations. Full replica would be a Python sidecar under TraceMind that runs QLoRA overnight on `~/.tracemind/traces.jsonl`.

**Why cut from H2.** Explicitly deferred by the charter (Pillar 1 + Non-Goals). GEPA replaces LoRA for the retrieval use case with no compute cost and no cloud. The remaining use case for LoRA — *generation style* personalization — is off-wedge for a memory OS.

**Revisit trigger.** If the Pillar 1 Q4 exit gate fails on the "generation quality" axis (which is *not* in the current gate but might be added if user feedback surfaces it), revisit LoRA for generation only. Never for retrieval.

**Estimated cost.** ~3000 LOC + Python bridge + safe-tensor storage + model download UX. 2027 at earliest.

---

### 3.11 Constitutional / value-conditioned retrieval

**What it is.** Retrieval is conditioned not just on the query but on a user-declared "constitution" (what they care about, what they want suppressed). E.g., "surface commitments to family before commitments to work between 6pm and 9pm."

**Why cut from H2.** VerbAffinityModel (Q4.12) captures behavioral patterns implicitly. Explicit constitutions would need UI, conflict resolution, and a much more elaborate policy layer. Overkill for H2.

**Revisit trigger.** If users, after seeing the VerbAffinityModel personalization, ask "can I tell it what I care about directly?" — that's a demand signal for constitutional retrieval.

**Estimated cost.** ~1000 LOC + Tauri settings surface. 2027.

---

### 3.12 Cross-user (federated) reflection sharing

**What it is.** Reflections about *how* to improve retrieval (not user data, just meta-learnings) shared across users. Federated learning at the Curator (L3) level.

**Why cut from H2.** Sharing anything cross-user violates the local-first thesis and would recreate the cloud-personalization category we explicitly reject. Even if the *content* is de-user'd meta-learning, the *fact of sharing* is off-wedge.

**Revisit trigger.** Never, unless a user explicitly opts into a shared "TraceMind community learnings" tier with full transparency about what leaves the device.

**Estimated cost.** Full trust and compliance surface. Not on any roadmap.

---

### 3.13 Longer-context transformer as a component of retrieval

**What it is.** Instead of retrieval → generation, use a long-context model (128k+) to read a slice of memory directly as context. Skip retrieval entirely for the long-context slice.

**Why cut from H2.** Local long-context models exist but are heavy and violate the ≤1.6GB active-tier budget. Also: retrieval quality is the moat; skipping it undermines Pillar 4's benchmark story.

**Revisit trigger.** If on-device long-context models drop below the tier budget (unlikely by end of 2026), reconsider as a *fallback* for queries the ComposedIndex flags as low-confidence.

**Estimated cost.** ~500 LOC for the fallback wiring, plus the model itself. 2027.

---

## 4. Open Threads (research, not implementation)

Ideas that need thinking before they're even backlog items.

- **What is the "right" evaluation metric for the recursive loop itself?** Q4 exit-gate conjunct 2 uses "rollouts per +1 F1" as a proxy. Better options may exist (regret bounds, KL from a baseline policy, held-out task diversity). Worth 1–2 sessions of research before Q4.
- **Should the User-Behavior Model (Q4.12) be a first-class memory (as charted) or a separate sidecar predictor?** First-class fits the wedge; separate is easier to reason about safety-wise (rollback of a memory is different from rollback of a predictor). Undecided.
- **How do we handle prompt-injection surfaces in the Reflexion log?** Charter R7 flags this; Q3.8 shipped without a hardened defense. Needs a threat model doc before Q4.11 Curator reads reflections in bulk.
- **Is contradiction-rate a metric or a UX signal?** Charter treats it as both. If the two diverge (e.g., users prefer *some* contradiction because it flags their own thinking), we have a spec bug. Need user-study confirmation.
- **AIE 2026 talks we didn't get to synthesize yet** — several from tracks like Emerging Models, Voice/Realtime, and Agent Reliability were in the `AIE_WorldsFair_2026_Agenda.xlsx` but never fed the charter. Worth a follow-up watch/read pass.

---

## 5. How to use this document

- **Before a new sprint**: check §3 for backlog items that unblock the sprint's goals.
- **When a charter item fails or gets rejected in review**: check §3 for alternatives.
- **When AIE 2027 planning starts**: §3.10, §3.11, §3.9, §3.1 are the natural 2027 headline candidates.
- **When memory is updated**: cross-link entries here from `~/.claude/projects/.../memory/*` so future sessions inherit the backlog rather than rediscovering it.

---

*Companion to `docs/CHARTER-H2-2026.md`. Not authoritative for planning; the charter is. This is the lineage layer.*
