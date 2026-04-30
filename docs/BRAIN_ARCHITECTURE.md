# TraceMind — internal architecture, learning, world model, and next routes

**Status**: proposal / brainstorm — wedge sharpened 2026-04-27
**Date**: 2026-04-27 (updated)
**Predecessor**: `docs/PHASE4_DELIGHT.md`
**Companion (canonical wedge spec)**: `docs/INTENT_SYSTEM.md`
**Replaces (in spirit)**: the size-capped, F1-chasing parts of `ROADMAP.md`

> **Wedge update 2026-04-27:** TraceMind is a **system of intents** —
> it captures what you commit to, the context you had when you
> decided, and what actually happened, then anticipates the
> decisions you're about to make from your own track record. The
> primitive is `Commitment` (intent + decision + hypothesis as one
> shape), and the system surfaces predictions in three layers
> (L1 silent prefetch → L2 visible patterns → L3 opt-in
> recommendations). The full data model, state machine, world-model
> training contract, and safety rules live in `docs/INTENT_SYSTEM.md`.
> This document remains the engineering-internal architecture map
> and overall sprint frame.

> **Framing note (confirmed):** *externally* we don't market this as a
> "brain" or any single metaphor. The user-facing positioning line is
> *"a system of intents — captures what you commit to, learns how it
> plays out, anticipates what's coming next, all on your machine."*
> *Internally* we structure the system as close to how a brain factors
> cognition as we can, because that's the architecture most likely to
> actually help someone think. Brain language stays in the
> engineering docs.

---

## 0. What changed in the brief

### 0.1 Wedge update (2026-04-27)

The wedge is now **system of intents** (full spec:
`docs/INTENT_SYSTEM.md`). Concretely:

- New primitive: `Commitment` (Intent | Decision | Hypothesis), with
  `Outcome` and `Anticipation` as companions. Lives in a new
  `tm-intent` crate.
- New MCP tools: `memory_commit`, `memory_resolve`, `memory_recommend`.
- New surfaces: implicit-mining of commitments from existing capture,
  daily-brief surfacing of open intents and resolved outcomes,
  pattern detector for cross-commitment statistics.
- World model gets a sharp training target: predict outcome polarity
  given (statement, context, chosen option). No more vague "predict
  next topic" — that's the L1 prefetch use; L2/L3 use a real
  outcome-prediction head.
- Three predictive layers with strict opt-in gating — L1 prefetch
  always on (silent), L2 pattern surfacing opt-in after 30 days
  / 50 commitments, L3 conversational recommendations require
  explicit opt-in *and* world-model accuracy gate.

Sprints B / C / E / G have been re-aimed; A / D / F unchanged in
scope.

### 0.2 Decisions confirmed by Aaditya 2026-04-26 / 2026-04-27:

1. **Internal architecture is brain-shaped; external metaphor is open.**
   The factoring (working / episodic / semantic / procedural memory,
   default-mode reflection, reward circuit, etc.) is how the system
   is built, but we'll use whatever surface metaphor resonates with
   each persona/audience. No "second brain" hard-branding.
2. **Crate-size / RAM cap dropped.** Quality is the constraint. The
   promise is "seamless on every device the user has" — laptop and
   mobile — solved with platform-specific tiers (small distilled model
   on phones, full Tier-1 on laptops, Apple FoundationModels on iOS
   26+).
3. **Wedge = solo founder for *defaults*, three modes at 1.0.**
   Used to break ties on onboarding / brief tone / persona pack.
   Engine stays universal. §3.
4. **Optional encrypted-cloud Tier for low-end devices: opt-in only.**
   Off by default; user-initiated; never required. §4.1.
5. **Multimodal capture is critical on every platform.** §5 has been
   re-bunched: screenshots / voice ingest / photos move into early
   sprints alongside core UX, not held back as a late "sensory
   cortex" sprint.
6. **System of intents is the wedge.** Forward intents +
   backward decisions are one primitive (`Commitment`). System
   captures, attaches outcomes, learns patterns, anticipates next
   decisions. See `docs/INTENT_SYSTEM.md`.
7. **Three predictive layers (L1 / L2 / L3) with opt-in gating.**
   Prediction is the front door; search is the audit trail. Every
   anticipation cites its grounding commitments and is dismissable.

---

## 1. Internal architecture: cognitive subsystems → existing crates

This section is *engineering-internal*. It uses brain language because
the factoring matters for the next sprints, not because we'll ship
that vocabulary in the product. Most of the parts already exist —
they were just named after database concepts. Renaming-in-our-heads
+ a few new pieces gives us a coherent cognitive layout.

```
                  ┌──────────────────────────────────────────────┐
                  │                  PERCEPTION                  │
                  │     tm-capture · tm-ingest · tm-voice(N)     │
                  └─────────────────────┬────────────────────────┘
                                        │  (signals, turns, voice)
                  ┌─────────────────────▼────────────────────────┐
                  │              WORKING MEMORY (N)              │
                  │   in-RAM ring of last N turns + retrievals   │
                  │   tm-types::WorkingMemory  (new)             │
                  └─────────────────────┬────────────────────────┘
                                        │
       ┌────────────────────────────────┼────────────────────────────────┐
       │                                │                                │
┌──────▼─────────┐              ┌───────▼────────┐              ┌────────▼───────┐
│  EPISODIC      │              │  SEMANTIC      │              │  PROCEDURAL    │
│  tm-episodic   │              │  tm-graph      │              │  tm-episodic   │
│  (JSONL turns, │              │  (entities,    │              │   ::Procedure  │
│   trajectories)│              │   triples,     │              │   Store        │
│                │              │   communities) │              │                │
└──────┬─────────┘              └───────┬────────┘              └────────┬───────┘
       │                                │                                │
       │       ┌────────────────────────┴────────────┐                   │
       │       │  HIPPOCAMPUS (consolidation)        │                   │
       │       │  tm-reason::Consolidator            │                   │
       │       │  + tm-reflect (new) — dreaming,     │                   │
       │       │    deductive/inductive promotion    │                   │
       │       └────────────────────────┬────────────┘                   │
       │                                │                                │
       └────────────────────────────────┼────────────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │           PREFRONTAL CORTEX                  │
                  │   tm-controller (planner, bandit)            │
                  │   tm-retrieval (14-phase pipeline)           │
                  │   tm-reason (chains, analogy, causal)        │
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │            DEFAULT MODE NETWORK              │
                  │  tm-reflect (new): idle pattern detection,   │
                  │  loose-end finder, daily-brief generator     │
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │            LANGUAGE / NARRATIVE              │
                  │  tm-narrate (new) — prose synthesis,         │
                  │  citations, voice (`tm-companion` persona)   │
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │             REWARD CIRCUIT                   │
                  │  tm-controller (LinUCB) — short-term         │
                  │  tm-preference (new) — medium/long-term      │
                  │  feedback ring buffer (already in tm-capture)│
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │      INTENT / DECISION STORE (the wedge)     │
                  │  tm-intent (new) — Commitment + Outcome      │
                  │   primitives, state machine, miner           │
                  │  tm-capture::CommitmentMiner — implicit mine │
                  │   commitment-shaped phrases from streams     │
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │       WORLD MODEL (predictive prior)         │
                  │  f_topic — next-topic embedding (L1)         │
                  │  f_outcome — polarity logits (L2/L3)         │
                  │  trained nightly in tm-reflect, per-user     │
                  └─────────────────────┬────────────────────────┘
                                        │
                  ┌─────────────────────▼────────────────────────┐
                  │         ANTICIPATION GENERATOR               │
                  │  L1 PrefetchQuery (silent, always on)        │
                  │  L2 PatternMatch (visible, opt-in)           │
                  │  L3 Recommendation (conversational, opt-in)  │
                  │  every Anticipation cites grounding          │
                  │  Commitments, calibrated, dismissable        │
                  └──────────────────────────────────────────────┘
```

### 1.1 Memory types

| Brain concept       | TraceMind crate                       | Status                                                      |
|---------------------|---------------------------------------|-------------------------------------------------------------|
| **Working memory**  | *new*: `tm-types::WorkingMemory`      | Sprint B — last N turns + retrievals, ring buffer           |
| **Episodic memory** | `tm-episodic` (JSONL traces)          | exists — ingest/retrieve appended; trajectories with rewards|
| **Semantic memory** | `tm-graph` + `tm-vector`              | exists — entity/triple store, embeddings, communities       |
| **Procedural**      | `tm-episodic::ProcedureStore`         | exists — named procedures, JSONL lifecycle                  |
| **Sensory buffer**  | `tm-capture`                          | exists — clipboard / shell / MCP intake, feedback ring      |
| **Intent / decision store (the wedge)** | *new*: `tm-intent` | Sprint B — `Commitment` + `Outcome` + `Anticipation` |

### 1.2 Cognitive operations (mostly exist, some new)

| Brain concept        | Where it lives                                              | What it does                                                      |
|----------------------|-------------------------------------------------------------|-------------------------------------------------------------------|
| **Attention**        | `tm-controller::QueryPlanner`                               | picks bandit arm, decomposes query, allocates retrieval phases    |
| **Recall**           | `tm-retrieval::RetrievalEngine` (14 phases)                 | vector + graph + episodic + ColBERT + RRA + MMR + temporal        |
| **Association**      | `tm-reason::AnalogySolver` (WL kernel)                      | structural similarity across subgraphs                            |
| **Reasoning**        | `tm-reason::ChainBuilder`                                   | graph-of-thought chains with causal trace                         |
| **Consolidation**    | `tm-reason::Consolidator`                                   | decay, prune, merge                                               |
| **Dreaming**         | *new*: `tm-reflect`                                         | nightly deductive/inductive promotion + outcome attachment        |
| **Pattern detection**| *new*: `tm-reflect::PatternDetector`                        | per-cell stats over `Completed` Commitments → L2 anticipations    |
| **Synthesis (talk)** | *new*: `tm-narrate` (Tier-1 LLM)                            | prose answers with citations, in `tm-companion` voice             |
| **Self-monitoring**  | `tm-episodic::TrajectoryStore` + per-arm reward             | "did the last query work?" feedback to LinUCB                     |
| **Preference**       | *new*: `tm-preference`                                      | learns "what kinds of answers does this user keep?"               |
| **Salience**         | *new*: surprisal score in `tm-reflect`                      | "what's worth dreaming about?" (driven by world-model surprise)   |
| **Commitment mining**| *new*: `tm-capture::CommitmentMiner`                        | regex+heuristic phrase mine → candidate `Commitment` records      |
| **Outcome matching** | *new*: `tm-reflect::OutcomeMatcher`                         | embedding-similarity match between later captures and open commits|
| **Anticipation**     | *new*: `tm-reflect::AnticipationGenerator`                  | emits L1/L2/L3 predictions; world-model + pattern-detector driven |
| **World modelling**  | *new*: `tm-reflect::WorldModel` (`f_topic` + `f_outcome`)   | predicts next-topic embed (L1) and outcome polarity (L2/L3)       |

### 1.3 Context graphs — what they actually are

The graph in `tm-graph` is one graph, but it's used as five overlapping
*context graphs* simultaneously. Naming them explicitly clarifies what
each retrieval phase is asking for:

1. **Topical graph** — entities + their typed edges (`mentions`,
   `references`, `derived_from`). Used by k-hop expansion.
2. **Temporal graph** — entities indexed by `created_at` + the access
   log. Used by temporal queries ("last week").
3. **Causal graph** — `tm-reason::CausalTrace` edges (`A_caused_B`,
   `A_blocked_B`). Used by reasoning narratives.
4. **Affective graph** — *new*: edges weighted by user thumbs-up/down
   and dwell-time signals. Used by preference-aware retrieval.
5. **Commitment graph** — *new*: `Commitment ── derived_from ──►
   Commitment`, `Commitment ── about ──► Entity`,
   `Outcome ── evidenced_by ──► Trace`. Used by the pattern detector
   and L2/L3 anticipation grounding.

The same node lives in all five graphs; only the edge layer changes.
This is the same trick the brain plays — the same place encodes a
fact, a date, a cause, an emotion, *and* a commitment chain.

---

## 2. How the brain learns (feedback + experience)

The learning story today is one short loop. The plan below is four
loops at different timescales. Each one already has a partial
implementation; nothing here is greenfield.

### 2.1 Loop 1 — Bandit (seconds)

**What it does**: every retrieval picks one of 5 arms; reward = how
much the user used the result.

- **Signal**: implicit (entity click-through, dwell), explicit (👍 / 👎),
  and proxy (did the user re-ask within 60s? = bad).
- **Algorithm**: LinUCB over context vector (query length, entity
  count, recency, store richness).
- **Where it lives**: `tm-controller::LinUCB` + `tm-controller::Bandit`.
- **What's new**: extend context vector with (a) working-memory
  state ("user is on a thread"), (b) Tier (0/1/2) being used, (c)
  affective-graph signal density on the query topic.
- **Persistence**: `~/.tracemind/bandit.json` + `linucb.json`.

### 2.2 Loop 2 — Preference model (hours → days)

**What it does**: learns a 64-dim preference embedding per user that
biases ranking. "This user prefers terse / cited / counterfactual /
positive-toned answers."

- **Signal**: thumbs-up answers → positive examples; thumbs-down →
  negative; corrected answers → negative + the correction is positive.
- **Algorithm**: triplet contrastive loss on
  `(query, kept_answer, rejected_answer)` triples. ~1k params, trained
  online on CPU. Or even simpler: weighted averaging of embeddings for
  liked / disliked answers.
- **Where it lives**: *new* `tm-preference` crate (~300 LOC).
- **Effect**: small re-ranking nudge applied after RRA, before MMR.
  Never overrides relevance — it tilts ties.
- **Privacy**: 100% local. The model is per-user, persisted to
  `~/.tracemind/preference.bin`.

### 2.3 Loop 3 — Salience model (days → weeks)

**What it does**: learns *what's worth remembering*. Today every
clipboard event becomes a signal. That's noise. The salience model
predicts, before NER/embed, whether a capture is worth promoting.

- **Signal**: a capture is salient iff it later (a) gets queried
  against, (b) gets surfaced in a daily brief and the user clicks, or
  (c) participates in a kept reasoning chain.
- **Algorithm**: logistic regression over capture features (length,
  topic novelty via cosine to existing centroid, time-of-day, source
  app). Trained nightly in `tm-reflect`.
- **Effect**: low-salience captures stay in the fast-path lake but
  don't get promoted to the graph during consolidation. Saves
  graph-store bloat. Configurable threshold.
- **This is the surprise score** that feeds dreaming (TM-5.1-004).

### 2.4 Loop 4 — Counterfactual reasoning (weekly+)

**What it does**: "If TraceMind had retrieved differently a week ago,
would the answer have been better?" — runs offline against the
trajectory store.

- **Signal**: stored trajectory contains the chosen arm + reward; we
  re-run the same query offline with each non-chosen arm and score the
  result against the user's eventual feedback (or against a Tier-2
  judge model).
- **Algorithm**: doubly-robust off-policy evaluation (already a stub
  in `tm-controller::OffPolicyEvaluator` per `tm-types::Trajectory`).
- **Effect**: bandit learns from imagined alternatives, not just the
  paths it actually took. Roughly equivalent to dreaming-while-awake
  for the reward circuit.
- **Cost**: runs nightly, cap to 100 trajectories per pass.

### 2.5 The full feedback flow

```
   user query                              user reaction
       │                                          │
       ▼                                          ▼
   plan + arm select  ──►  retrieve  ──►  narrate  ──►  ✋ thumbs / dwell
       │                       │              │              │
       │                       │              │              │
       ▼                       ▼              ▼              ▼
   trajectory  ◄────────  signal_hits  ◄──  citations  ◄───  reward
       │                       │              │              │
       └───────────────────────┴──────────────┴──────────────┘
                                  │
                                  ▼
                ┌─────────────────────────────────────┐
                │   Loop 1: LinUCB (seconds)          │
                │   Loop 2: preference (hours-days)   │
                │   Loop 3: salience (days-weeks)     │
                │   Loop 4: counterfactual (weeks+)   │
                └─────────────────────────────────────┘
```

All four loops are local. None needs the cloud. The reward circuit
*is* the differentiator versus stateless RAG; we just have to actually
ship loops 2–4.

---

## 3. Wedge persona — plain language

**Wedge persona** = the *first specific kind of user* we make
spectacularly happy, knowing the product will eventually serve more
people. It's a starting wedge into a market — not a final ceiling.

Examples from products we know:

| Product       | Wedge persona (start)                | Eventual market               |
|---------------|--------------------------------------|-------------------------------|
| Stripe        | Two technical co-founders shipping a SaaS in YC | every internet business |
| Notion        | Small startup teams who hated wikis  | half the knowledge-work world |
| Superhuman    | VCs and execs who lived in Gmail     | anyone serious about email    |
| Figma         | Two-person product teams sharing UI work | every designer and PM     |
| Obsidian      | Power users on Reddit who wanted local Markdown | ~2M PKM enthusiasts |

The wedge isn't "we only sell to founders forever." It's "if we make
*this group* love us, the rest follows." It compresses the design
space — every UX choice becomes "what does my wedge user need?" That
discipline is what stops the product from being mush.

### 3.1 What this means for TraceMind

If we don't pick a wedge, the product is "memory for everyone," and
"everyone" means we end up averaging across mutually contradictory
needs (devs want APIs, PhDs want citations, journalers want privacy
+ tone, founders want speed + voice). The result is a mush no group
loves.

The recommendation in `PHASE4_DELIGHT.md` was **solo founders / indie
operators**, because (a) Aaditya is one — tight dogfood loop, (b) the
behaviors line up (capture obsessively, value privacy, will pay), and
(c) the competitors all skew developer-platform.

But you flagged this — *quality should not depend on the user being a
specific kind*. That's right too. The compromise:

- **Wedge user for *design* decisions**: solo founder / indie operator.
  Used to break ties on UX, tone, default brief style, voice persona.
- **Quality bar for *engineering*: universal**. Voice mode works for
  any English speaker. Capture works on any text. Graph view works
  for any topic. We don't ship founder-only logic in the core; we
  ship founder-shaped *defaults* that anyone can override.

In practice: the persona shapes the **first-run experience, the
default brief tone, and the marketing landing page**. It does not
shape the retrieval engine, the graph schema, or the learning loops.
Those stay general.

### 3.2 If we genuinely want broad appeal first

There's a defensible alternate path: skip the wedge, position as
**"the second brain that's actually yours"** with three pre-built
"modes" (Founder / Researcher / Journaler). Each mode = a
`tm-companion` persona pack + brief template. Same engine.

This is more work upfront (three voice/tone packs instead of one) but
avoids the "is this for me?" problem at install. If you'd rather do
this, the tech is identical — we just ship three persona configs
instead of one.

**My recommendation still leans wedge-first, three-modes-at-1.0**.
But it is your call.

---

## 4. Cross-platform: laptop + mobile, no compromises

The premise from your note: *"if its a quality gap crate size doesnt
matter as long as all users, mobile laptop or otherwise can use it
seamlessly."*

That changes the architecture in concrete ways. Here is the plan.

### 4.1 The platform tier

| Device class                             | Inference tier                    | Models bundled                | Active RAM cap |
|------------------------------------------|------------------------------------|--------------------------------|----------------|
| MacBook (M-series, 16GB+)                | Tier-1 default (Qwen 2.5 1.5B Q4)  | full                           | ~1.6GB         |
| MacBook (Intel / 8GB)                    | Tier-1 light (Qwen 0.5B Q4)        | small variant                  | ~700MB         |
| Windows / Linux laptop                   | Tier-1 default                     | full                           | ~1.6GB         |
| iOS 26+ iPhone / iPad                    | Apple FoundationModels (3B on-device, free) | Apple FM + Whisper-tiny | OS-managed     |
| iOS < 26 iPhone                          | Tier-1 light (Qwen 0.5B)           | small + Whisper-tiny           | ~600MB         |
| Android (high-end, 8GB+ RAM)             | Tier-1 light (Qwen 0.5B GGUF)      | small + Whisper-tiny           | ~600MB         |
| Android (mid/low-end)                    | Tier-0 + optional encrypted-cloud  | embedder only                  | ~150MB         |
| Old hardware / browser-only              | Tier-0 + optional encrypted-cloud  | embedder only                  | ~80MB          |

Key principle: **the *experience* (daily brief, voice, surprise
surfacing) is identical on every platform**. The *engine quality*
varies. Low-end devices get a "light brain" tier and the option to
opt into encrypted-cloud Tier-1 — never required, never default, but
available for the user who'd rather have quality than purity.

### 4.2 Sharing the engine — UniFFI

The 17 Rust crates compile to a static library. We expose them to
mobile via [UniFFI](https://github.com/mozilla/uniffi-rs):

- **iOS**: UniFFI emits Swift bindings → `TraceMindKit.framework` →
  used by a thin SwiftUI app. Whisper via `whisper.cpp` Metal build.
  Apple FM via the new `objc2-foundation-models` crate (when on
  iOS 26).
- **Android**: UniFFI emits Kotlin bindings → AAR → used by a Jetpack
  Compose app. Whisper via `whisper.cpp` (NNAPI).
- **Desktop**: existing `tm-cli`, `tm-mcp`, `tm-tauri` unchanged.

The LLM tier is platform-specific:

- llama.cpp (`llama-cpp-2` Rust binding) on macOS / Win / Linux / Android.
- Apple FoundationModels via Swift on iOS 26+.
- Tier-0 (no LLM) anywhere.

`tm-answer` adds a `Backend` trait the platform fills in.

### 4.3 Sync — the seamless promise

If the user has TraceMind on laptop + phone, the experience must feel
like one brain. Approach:

- **Storage of record**: the device that ingested the data (laptop is
  primary by default).
- **Sync layer**: end-to-end encrypted, using
  [Automerge](https://automerge.org/) CRDTs over a user-chosen
  transport (default: iCloud Drive / Google Drive folder; opt-in:
  Tailscale-style local network sync; never our servers).
- **Conflict resolution**: CRDTs handle most things. Contradictions
  (per TM-5.1-005) flow through the existing supersession path.
- **Encryption**: per-user keypair generated at first run, never
  leaves device. Backup phrase shown once.
- **Selective sync**: user picks what syncs (graph + daily briefs by
  default; raw captures can stay local-only).

This is a *new* crate, `tm-sync`, scoped for after the desktop product
hits PMF. It's not Sprint-1 — but designing the data layer with sync
in mind from now (UUID-keyed everything, append-only logs, no
location-dependent state) means we don't have to refactor when we get
there.

### 4.4 What changes in the existing code

- **Drop the `<200MB idle / <500MB active` constraint** from `CLAUDE.md`
  for Tier-1+. Replace with the table in §4.1.
- **Make `tm-answer::Backend` a trait**, with `LlamaCppBackend`,
  `AppleFmBackend`, `OnnxBackend`, `Tier0Extractive` impls.
- **Make all crate APIs `Send + Sync`** so they compose under iOS's
  Grand Central Dispatch + Android's coroutines. (Most already are.)
- **Remove direct filesystem assumptions** from `~/.tracemind/`. Let
  the platform pass a `DataDir` at init. iOS sandbox lives in
  `Documents/TraceMind/`, Android in app-private storage.
- **Push platform-native UI down to leaf crates only** (`tm-tauri`
  for desktop, future `tm-ios`, `tm-android`) — keep `tm-types`,
  `tm-graph`, `tm-vector`, `tm-retrieval`, `tm-reason` totally pure.

These are 1–2 weeks of refactor, mostly trait extractions and
pulling `~/.tracemind/` literals into config. Already partially done
via `TM_DATA_DIR` env override.

---

## 5. Redesigned roadmap — seven sprints (intent-aimed, multimodal redistributed)

Sprints are re-aimed around the system-of-intents wedge (see
`docs/INTENT_SYSTEM.md` for the canonical spec). Each sprint's
success metric is framed in terms of commitments, outcomes, and
anticipation accuracy — not generic memory quality. Multimodal
capture stays redistributed across A/B/C/F so no platform ever ships
text-only.

### 5.1 Sprint A — Real answers + first capture moat *(unchanged)*
**Theme**: stop pretending Tier-0 is the product. Make the LLM real,
and ship the highest-yield non-text capture path on day one.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| TM-5.2-002   | Wire `tm-answer::LocalLlmBackend` to llama-cpp-2 |
| TM-5.2-003   | NL answers → `NarrativeResponse` everywhere  |
| TM-5.1-002   | MCP structured ingestion (Claude/LLMs feed pre-extracted facts) |
| —            | Bundle Qwen 2.5 1.5B Q4 (laptop) + 0.5B Q4 (mobile/light) |
| —            | First-run installer downloads tier weights with progress UI |
| TM-5.2-010A  | **Obsidian vault import** (cheapest huge-ingestion moat) |
| TM-5.2-009   | File-import real embeddings (chore)          |
| TM-5.2-005   | LoCoMo regression run with Tier-1 — target ≥ 60 mini |

**Exit / Felt**: prose answers with citations. Tier-1 default. Power
users can populate the graph from an Obsidian vault in minutes.
LoCoMo F1 ≥ 60. *"Answers stop being a database."*

### 5.2 Sprint B — Commitment primitive + L1 prefetch + voice
**Theme**: the wedge primitive ships. The system captures what you
commit to and pre-warms what it expects you'll ask. Voice is part of
the core loop from day one.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| —            | New crate `tm-intent` (Commitment / Outcome / Anticipation data model + state machine) |
| —            | MCP tools `memory_commit`, `memory_resolve` (see INTENT_SYSTEM §8) |
| —            | `tm-capture::CommitmentMiner` — implicit phrase mining (regex + LLM normalization) |
| —            | `tm-types::WorkingMemory` ring buffer        |
| —            | New crate `tm-reflect` (skeleton + outcome-prompt scheduler) |
| TM-5.1-004   | Offline dreaming (deductive/inductive promotion) |
| —            | Daily-brief generator (`tm-reflect` + `tm-narrate`) — surfaces open commitments |
| TM-5.2-001   | Tauri packaging (panel for brief)            |
| —            | New crate `tm-voice` — Whisper-tiny STT + Piper TTS |
| —            | **Voice ingest** (⌘⇧Space hold-to-record → CommitmentMiner) |
| —            | **Voice playback** of brief + replies         |
| —            | World model v0: `f_topic` MLP, contrastive nightly training |
| —            | **L1 Prefetch layer** wired into `tm-retrieval` (silent, always on) |

**Exit / Felt**: *"I can talk to it. It captured what I committed to.
The morning brief lists my open intents."*

### 5.3 Sprint C — Outcome attachment + pattern detector + screens/web
**Theme**: the system closes the loop on commitments and starts
*noticing patterns*. It thinks while you're not looking, and sees
what you see.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| —            | `tm-reflect::OutcomeMatcher` — implicit text matcher + time-triggered outcome prompt |
| —            | `tm-reflect::PatternDetector` — Wilson-LB 95% gate, n≥6, |lift|≥0.25 |
| —            | **L2 Pattern surfacing** — `AnticipationKind::PatternMatch` in brief (opt-in after 30 days / 50 commitments) |
| TM-5.1-001   | Two-speed ingestion (signal lake + consolidation) |
| TM-5.1-003   | Observation hierarchy (explicit/deductive/inductive/contradiction) |
| TM-5.1-005   | Contradiction detection + supersession       |
| TM-5.2-004   | Proactive surfacing ("I noticed…")           |
| —            | Salience model (Loop 3) — consumes f_topic surprise score |
| —            | **Screenshot capture + VLM caption** (Phi-3.5-vision or Moondream) |
| TM-5.3-005   | **Browser extension** (Chrome/Firefox highlight capture) |
| —            | Calibration panel v1 (read-only — outcome match rate, pattern accuracy) |

**Exit / Felt**: *"It noticed the same thing went sideways three
times — and it's right."* TraceMind volunteers observations;
multimodal capture is real on the laptop.

### 5.4 Sprint D — Reward circuit (preferences + counterfactuals)
**Theme**: the system *learns from its own mistakes* and starts
running counterfactuals against past commitments.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| —            | New crate `tm-preference` (Loop 2 — preference embedding) |
| TM-5.3-004   | Capacity-aware bandit features (store richness in LinUCB) |
| —            | Feedback ring buffer → preference model wiring |
| —            | Counterfactual replay (Loop 4) in `tm-reflect` — uses `f_outcome` rollouts over commitment history |
| TM-5.2-006   | Iterative/agentic retrieval (uses preference signal) |
| TM-5.1-006   | Context-budget allocator (uses preference for tie-break) |
| —            | Pattern detector reads preference signal; user-silenced cells drop out |

**Exit / Felt**: *"It's getting my taste. The brief is shorter and
sharper than week one."*

### 5.5 Sprint E — Visible surface + L3 conversational layer (gated)
**Theme**: the desktop app gets a face, and recommendation surfaces
land — *only after the world model passes a per-user accuracy gate*.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| —            | Tauri **commitment timeline** + intent dashboard |
| —            | Brief archive + memory garden as secondary tab |
| TM-5.2-007   | Global hotkey ⌘⇧M (quick capture / search)   |
| —            | First-run onboarding (sample data → meaningful brief in 60s) |
| TM-5.2-008   | Performance benchmarking + footprint baselines |
| —            | MCP tool `memory_recommend` + **L3 Recommendation surface** (opt-in, accuracy-gated) |
| —            | Calibration panel v2 (interactive — silence patterns, reset model) |
| TM-5.2-010B  | **Obsidian plugin** (TS, in companion repo)  |

**Exit / Felt**: *"I can see my own track record at a glance. When I
ask 'what would you do', it shows me what I did."* Public alpha.

### 5.6 Sprint F — Mobile + photos + cross-device sync
**Theme**: the system follows you. Mobile gets full multimodal
parity; photos are natural on mobile.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| —            | UniFFI bindings — Swift + Kotlin             |
| —            | iOS app (SwiftUI) — capture, brief, voice, commitment timeline |
| —            | Android app (Compose) — same                 |
| —            | Apple FoundationModels backend (iOS 26+)     |
| —            | Distilled Qwen 0.5B Q4 mobile bundle         |
| —            | **Photo ingest** (EXIF + VLM caption) — phones first, desktop inherits |
| —            | New crate `tm-sync` (Automerge over iCloud / Drive folder) |
| —            | Optional encrypted-cloud Tier (opt-in only) for low-RAM Android / browser |
| —            | L1 prefetch on mobile                        |

**Exit / Felt**: *"It follows me. Voice-captured intent on the train
shows up in the laptop brief that night."*

### 5.7 Sprint G — World model v2 + scale
**Theme**: the predictive prior matures — `f_outcome` ships, surprise
scoring is in CI, and the system generalizes across years.

| Existing ID  | Title                                       |
|--------------|---------------------------------------------|
| TM-6.0-001   | **`f_outcome` transformer** replaces v0 MLP (per §7) |
| —            | JEPA-style surprise scoring + held-out evaluation in CI |
| —            | Calibration auto-quiets bad models (per-user gate) |
| TM-5.3-002   | Artifact-aware retrieval pruning             |
| TM-5.3-003   | Artifactless-copy evaluation                 |
| TM-5.3-006   | Optional GLiNER micro-NER                    |
| TM-5.3-007   | HNSW vector index (past 100k entities)       |
| TM-5.3-001   | Multi-agent peer paradigm                    |
| TM-6.0-002   | SSM/Mamba history compression                |

**Exit / Felt**: *"It's predicting outcomes and it's right more than
wrong."* Scales to 1M+ captures per user without slowdown.

### 5.8 Things that drop out

- **Hard idle-RAM cap** — gone (replaced by §4.1 platform table).
- **TM-6.0-003 multi-user ACL** — pushed past 1.0; not on the
  founder/researcher/journaler wedge path.
- **Old "Sprint G multimodal sensory cortex"** — dissolved into B/C/F.
- **L3 conversational recommendation before E** — gated behind
  per-user accuracy threshold; never default-on.

### 5.9 Order rationale

The order optimizes for *user-felt aliveness per sprint*, with the
intent primitive landing in B (right after answers become real):

- A: answers stop being a database (felt: "this is real")
- B: commitment + voice + L1 (felt: "it talked to me, and it knows what I committed to")
- C: outcomes + patterns (felt: "wait, how did it know?")
- D: personal taste + counterfactuals (felt: "it knows me")
- E: visible product + L3 gated (felt: "this is beautiful — and it's right")
- F: portable + photos (felt: "it goes with me")
- G: scale + foresight (felt: "it predicted that")

E before F is deliberate: the desktop is the flagship that proves
the pattern before we ship two more apps. Multimodal is in B/C/F so
no platform ever ships text-only. **L2 surfaces are gated on 30
days / 50 commitments; L3 is gated on a calibration accuracy
threshold** — both per-user, both visible in Settings.

---

## 6. What I need from you (decision log)

Open questions, in order of blocker-ness:

1. **Internal architecture is brain-shaped, external metaphor stays
   open.** ✅ Confirmed. Engineering uses the cognitive vocabulary
   (working memory, hippocampus, default mode network, reward
   circuit). Marketing / website / onboarding pick the metaphor that
   resonates per audience. We don't hard-brand "second brain."
2. **Wedge = solo founder for defaults, three modes at 1.0.**
   ✅ Confirmed. Engine universal; persona shapes onboarding /
   default brief tone / marketing landing. Founder / Researcher /
   Journaler persona packs ship at 1.0.
3. **Drop the `<200MB / <500MB` cap.** ✅ Confirmed. Per-platform
   tier table in §4.1 governs.
4. **Optional encrypted-cloud Tier — opt-in only.** ✅ Confirmed.
   Off by default. Available for low-RAM Android / browser tier
   users who want quality-over-purity. Never required, never
   silent, always disclosed. Lives behind a single `tm-cloud`
   feature flag the user can toggle off forever.
5. **Multimodal critical on every platform.** ✅ Confirmed.
   §5 redistribution: voice in/out lands in Sprint B, screenshots
   + browser capture in Sprint C, photos in Sprint F (mobile-natural
   then desktop-inherits). No platform ever ships text-only.

---

## 7. Should we build a *world model* for the user?

**Short answer**: yes — narrowly scoped, two-headed (`f_topic` +
`f_outcome`), as the internal predictive prior that powers the three
opt-in surfaces (L1/L2/L3) of `INTENT_SYSTEM.md`. It is not a
user-facing "what TraceMind thinks you'll do" feature.

### 7.1 What a world model is, in this context

A world model is a pair of learned functions trained per-user on
their own trajectory + commitment history. See `INTENT_SYSTEM.md §7`
for the full contract; summarized here:

**`f_topic`** — given context (working memory + recent captures +
topical embedding), predicts the *latent embedding* of the next
likely topic / entity / question. Small MLP, ~600k params,
contrastive InfoNCE loss vs random negatives. Ships in Sprint B (v0).

**`f_outcome`** — given a `Commitment` + its `ContextSnapshot`,
predicts a polarity logit over `{good, neutral, bad}` outcome.
4-layer transformer, ~3M params, supervised on the user's own
resolved commitments. Ships in Sprint G (v2).

The two heads feed **three product hooks**:

1. **Surprise score** (L1 / salience) — `f_topic` prediction error =
   $\lVert f_\text{topic}(\text{ctx}) - \text{actual}\rVert$. High
   error = "this is unexpected" = high salience = boost ingestion
   priority. This is the right way to build the salience model
   in §2.3.
2. **Predictive priors for retrieval** (L1 prefetch) — `f_topic`
   gives a pre-warmed list of "topics I expect them to ask about
   today" → `tm-reflect` pre-fetches answers before the user types.
   Always-on, silent, no UI surface.
3. **Counterfactual scaffolding** (L2 patterns / L3 recommend +
   Loop 4) — `f_outcome` rollouts simulate "if this commitment had
   chosen option B, what's the predicted polarity?" The pattern
   detector consumes these for `AnticipationKind::PatternMatch`,
   and L3 `memory_recommend` cites them.

### 7.2 Why this is *not* an anti-pattern (with caveats)

The anti-pattern would be shipping a product feature called
*"TraceMind predicts your next move"* or *"your digital twin"* — that
overclaims, hallucinates, and feels invasive even when local. We're
not doing that.

What we *are* doing is using a world model the way real brains do:
as a silent background predictor whose only outputs are (a) *surprise
scores* on incoming captures, (b) *priority weights* for what to
surface in the brief, (c) *outcome polarity logits* over the user's
own commitments, and (d) *latent simulations* for the off-policy
evaluator. The user sees these only through L2 / L3 surfaces, both
of which are opt-in and accuracy-gated.

Concretely (hard rules, mirrored from `INTENT_SYSTEM.md §7.5`):

- **Embedding-space only.** Both heads predict in embedding /
  logit space, never in token/text space. The model cannot generate
  natural language about the user's life — it can only compute "is
  this surprising / on-pattern / off-pattern" or "this commitment's
  predicted polarity is X."
- **Bounded scope.** `f_topic` predicts next-topic, not next-action.
  `f_outcome` predicts polarity over the user's own commitments,
  not their emotional state, not the actions of other people.
- **Local-only by definition.** Per-user, trained online, persisted
  to `~/.tracemind/world_model/{f_topic,f_outcome}.bin`. Never
  uploaded, never shared, no federation.
- **Inspectable.** "Why was this flagged surprising?" returns the
  cosine distance from `f_topic`'s prediction. "Why this
  recommendation?" returns the supporting `Commitment` IDs and the
  pattern's Wilson-LB score. Every surface cites.
- **Deletable.** Single command resets both heads to a fresh prior.
  The user's trail vanishes from the model.
- **Calibration-gated.** L2 and L3 surfaces are auto-quieted when
  the held-out outcome match rate falls below the user's calibrated
  threshold (visible in Settings).

### 7.3 Architecture (Sprints B + G)

| Head        | Sprint | Model                          | Params | Train signal                        | Output                          | Storage |
|-------------|--------|--------------------------------|--------|-------------------------------------|----------------------------------|---------|
| `f_topic`   | B (v0) | MLP                            | ~600k  | Contrastive (target vs negatives)   | next-topic embedding             | ~5MB    |
| `f_outcome` | G (v2) | 4-layer transformer            | ~3M    | Supervised on resolved commitments  | logits over {good, neutral, bad} | ~25MB   |

- **Training data**: drawn entirely from the user's `TrajectoryStore`
  (existing stub fields `predicted_outcome`, `actual_outcome_embedding`,
  `surprise_score` are now load-bearing) and `tm-intent::Commitment` /
  `Outcome` records.
- **Schedule**: nightly in `tm-reflect`. Inference <50ms on CPU.
- **Hooks → layers**:
  - `f_topic` surprise → Loop 3 salience + L1 prefetch
  - `f_topic` priors → L1 prefetch list in `tm-retrieval`
  - `f_outcome` rollouts → Loop 4 counterfactual replay + L2
    pattern strength + L3 `memory_recommend` citations

### 7.4 The hard line

We will not ship:
- a "your digital twin is X% confident you'll do Y" UI surface
- predictions in natural language (no token-space generation
  *about the user* — only retrieval and templated synthesis)
- predictions that show up unprompted on L2/L3 surfaces
- any predictive output the user can't audit, silence, or clear
- L3 surfaces before the per-user calibration gate passes

Those are the moves that turn "useful prior" into "creepy mirror."
Stay on the right side of that line and a per-user world model is
one of the biggest differentiators from cloud-AI-memory products
(which can't build per-user world models without aggregating data
centrally).

---

## 8. Why use TraceMind over cloud-AI-memory products?

By "cloud-AI-memory products" we mean the systems that keep a
persistent memory across chats but live entirely on someone else's
servers — ChatGPT Memory, Claude Projects/Memory, Gemini saved info,
Mem0 cloud, Honcho cloud, Rewind cloud sync, Notion AI memory, etc.
They are real competitors with real advantages we shouldn't dismiss
(zero install, smarter base models, work in the browser).

### 8.1 Where cloud wins

Honest concessions:

- **Raw model intelligence**. GPT-5 / Opus-class models will outsynth
  Qwen 2.5 1.5B for the foreseeable future. If your single criterion
  is "best paragraph" answer, cloud wins.
- **Zero-setup**. Sign in, go. We'll always have an installer,
  even at our most polished.
- **Cross-user pattern learning**. They can see "people who ingest X
  also ask Y" globally; we can't.

If the user's job is "have one persistent chat assistant inside one
vendor's app," cloud is fine. We don't compete there.

### 8.2 Where TraceMind wins

The seven differentiators that hold up in any honest comparison:

1. **Your data never leaves your machine** (unless *you* turn on the
   opt-in cloud Tier). Cloud products store every clipboard fragment,
   shell command, screenshot, and voice memo on someone else's
   servers — typically used to train future models, however
   euphemistically the ToS phrases it. For founders / lawyers /
   engineers handling proprietary work, this is non-negotiable.
2. **Multi-modal capture across the *whole* device**, not just one
   chat. We see clipboard, shell, browser, screenshots, photos, voice,
   Obsidian, MCP — unified into one graph. Cloud chat memory only
   sees what you typed into that chat.
3. **You own the brain.** Model weights, embeddings, graph, traces,
   captures — all on your disk. Export it, fork it, audit it, delete
   it in one command. With cloud memory you can request deletion
   and trust they did it.
4. **Substrate, not silo.** TraceMind's MCP server feeds *any* LLM
   tool (Claude Code, Cursor, ChatGPT Desktop, custom agents) the
   same memory. Cloud memory is locked to one vendor; switching
   providers means starting from scratch.
5. **Real per-user learning loops** (§2). The four-loop reward
   circuit, preference model, salience model, and counterfactual
   replay all train *on you, for you*. Cloud "memory" is mostly a
   key-value store of saved facts plus retrieval — there is no
   per-user reward model running in the background.
6. **No marginal cost.** Once installed, every query is free.
   Cloud memory grows your prompt size and therefore your monthly
   bill. As your trail grows, cloud gets *more* expensive — local
   gets cheaper per query (HNSW + caching).
7. **Air-gapped capable.** Compliance-relevant for legal / health /
   regulated work and useful in-flight, in cabins, in dead zones.
   Cloud is unusable without internet by definition.

### 8.3 The one-line positioning

> **"TraceMind is a system of intents. It captures what you commit
> to, the context you had when you decided, and what actually played
> out — then anticipates the decisions you're about to make using
> only your own track record. Nothing leaves your machine. Ever."**

The previous framing — *"Cloud AI memory is a saved-prompts feature.
TraceMind is a learning system that lives on your hardware…"* —
remains a valid alternate one-liner, but the system-of-intents
formulation is sharper: it names a category, names a primitive
(`Commitment`), and names the success metric (outcome match rate)
without ever overpromising.

That sentence rules out chasing them on raw IQ (we lose) and points
at the things only we can do. The wedge user (solo founder) feels
all seven differentiators acutely; the broader audience feels at
least 1, 2, 3, and 6 the moment they switch model providers.

### 8.4 Why this is actually winnable

The pattern in software history: when a category becomes "clouded"
by default, a privacy-preserving local-first version eventually
captures the durable / paying / power-user segment. Examples:
1Password vs. Google Password Manager. Obsidian vs. Notion. Linear
vs. Jira Cloud. Local-first doesn't dethrone the leader — it carves
out a 5-15% segment that *pays*, churns less, and tells everyone
they know. That segment, for AI memory, doesn't have a winner yet.

---

## 9. Closing

Internal factoring stays brain-shaped because that's the architecture
most likely to actually help someone think. External framing stays
metaphor-flexible — but the **named wedge is "system of intents"**
(see `INTENT_SYSTEM.md`). We had most of the cognitive subsystems
already (episodic, semantic, procedural, attention, association,
consolidation, trajectory rewards) — we're adding:

- a working-memory ring buffer (`tm-types::WorkingMemory`),
- a default-mode network (`tm-reflect`),
- a language cortex (`tm-narrate`),
- a **commitment / decision store** (`tm-intent`) as the wedge primitive,
- the longer learning loops (preference, salience, counterfactual),
- a two-headed embedding-space world model (`f_topic` + `f_outcome`)
  as a bounded predictive prior,
- and **three opt-in predictive surfaces** (L1 silent prefetch, L2
  pattern surfacing after 30d/50 commitments, L3 conversational
  recommendation behind a calibration gate).

Multimodal capture is real on day one of every platform. Mobile
ships in F via UniFFI. The size cap is gone; quality is the
constraint. Cloud-AI-memory is not our category — they own
chat-attached saved-prompts; we own *intent-aware substrate that
runs on your hardware*. That category is open, and the success
metric (outcome match rate) is built into the primitive itself.

Sprint A starts with wiring `tm-answer` to `llama-cpp-2`.
