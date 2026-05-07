# TraceMind — System of Intents

**Status**: spec / proposal — wedge confirmed 2026-04-27
**Date**: 2026-04-27
**Predecessor**: `docs/BRAIN_ARCHITECTURE.md`
**Companion to**: `docs/PHASE4_DELIGHT.md`

---

## 0. The wedge, in one paragraph

TraceMind is a **system of intents**: it captures what you commit to,
the context you had when you committed, and what actually happened
later — and then anticipates the decisions you're about to make,
using only your own track record. Not a memory store, not a
chatbot — a *compounding decision intelligence* that lives entirely
on your machine.

The unit is the **commitment**: a forward-leaning intent ("I will
ship Friday") and a backward-resolving decision ("I went with vendor
A") are two phases of the same primitive. The system observes them,
attaches outcomes, learns patterns, and starts to *predict* — first
silently (prefetch), then visibly (pattern surfacing), then
conversationally (recommendations), each layer gated behind explicit
opt-in.

This doc specifies the data model, capture flow, outcome attachment,
pattern detector, three-layer prediction surface, world model
training contract, safety rules, and sprint plan.

---

## 1. Data model

Three primary types. Live in a new crate `tm-intent` (split out so
state-machine logic and outcome detector aren't entangled with the
graph store).

### 1.1 Commitment

```rust
pub struct Commitment {
    pub id: Uuid,
    pub kind: CommitmentKind,           // Intent | Decision | Hypothesis
    pub statement: String,              // "ship v2 by Friday"
    pub made_at: DateTime<Utc>,
    pub horizon: Option<DateTime<Utc>>, // when we expect resolution
    pub context_snapshot: SnapshotRef,  // pointer into snapshot store
    pub options_considered: Vec<String>,
    pub chosen: String,                 // which option won (= statement for single-option)
    pub confidence: f32,                // user-declared 0..1
    pub expected_outcome: Option<String>,
    pub stakes: Stakes,                 // Low | Medium | High | Reversible
    pub state: State,
    pub outcome_id: Option<Uuid>,       // resolved Outcome record
    pub derived_from: Vec<Uuid>,        // chain: this supersedes / refines prior commitments
    pub tags: Vec<String>,              // user or LLM-assigned
    pub source: Source,                 // Manual | VoiceCapture | ImplicitMined | McpStructured
}

pub enum CommitmentKind {
    Intent,        // forward-leaning, hasn't been acted on yet
    Decision,      // already chosen / executed
    Hypothesis,    // a tentative belief to be validated ("I think Postgres will scale")
}

pub enum State {
    Open,          // active, no outcome yet
    Acted,         // user has acted on it but outcome not assessed
    Completed,     // outcome attached
    Abandoned,     // user explicitly walked away
    Superseded,    // a later Commitment in derived_from supersedes this
}

pub enum Stakes {
    Low,           // routine; pattern detector ignores unless asked
    Medium,        // default
    High,          // amplified weight in pattern detection + prediction
    Reversible,    // even if high stakes, the user can undo (drives different surfacing tone)
}

pub enum Source {
    Manual,        // CLI / UI form
    VoiceCapture,  // ⌘⇧Space + Whisper STT
    ImplicitMined, // detected from clipboard/shell/MCP turns by phrase mining
    McpStructured, // emitted by an LLM via memory_commit MCP tool
}
```

### 1.2 Outcome

```rust
pub struct Outcome {
    pub id: Uuid,
    pub commitment_id: Uuid,
    pub observed_at: DateTime<Utc>,
    pub description: String,
    pub polarity: Polarity,
    pub surprise: f32,                  // ||predicted_embed - actual_embed||
    pub evidence_traces: Vec<Uuid>,     // pointers to traces / signals that prove the outcome
    pub user_note: Option<String>,
    pub source: OutcomeSource,
}

pub enum Polarity {
    Better,        // outcome exceeded expected
    AsExpected,
    Worse,
    Mixed,         // some good, some bad
    NoOutcome,     // commitment ended without measurable result (legitimate state)
}

pub enum OutcomeSource {
    UserPrompted,    // brief asked "what happened with X?" → user answered
    ImplicitMatched, // detected via embedding similarity to later capture text
    McpStructured,   // explicit memory_resolve MCP call
}
```

### 1.3 Anticipation

The world-model output. First-class so we can score the model's
calibration over time.

```rust
pub struct Anticipation {
    pub id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub trigger: TriggerContext,        // what caused us to predict
    pub kind: AnticipationKind,
    pub predicted_commitment: Option<CommitmentDraft>,
    pub grounded_in: Vec<Uuid>,         // existing Commitments backing this prediction
    pub confidence: f32,
    pub surfaced_at: Option<DateTime<Utc>>,
    pub user_response: Option<UserResponse>,
    pub eventual_match: Option<Uuid>,   // did a real Commitment match this prediction?
    pub expires_at: DateTime<Utc>,      // hard expiry; auto-dismiss
}

pub enum AnticipationKind {
    PrefetchQuery,     // L1: silent — pre-warm retrieval
    PatternMatch,      // L2: visible — "this looks like a prior pattern"
    Recommendation,    // L3: opt-in — "what would you do?"
}

pub struct TriggerContext {
    pub working_memory_hash: String,    // recent N turns
    pub topic_centroid: Vec<f32>,       // embedding centroid of current focus
    pub time_of_day: u8,                // 0..23
    pub source_app: Option<String>,     // "claude-code", "browser", "obsidian"
    pub matched_phrase: Option<String>, // what we mined, if implicit
}

pub enum UserResponse {
    Accepted,        // user actioned the prediction
    Dismissed,       // user closed it (negative training signal)
    Starred,         // user explicitly liked it (strong positive)
    Silenced,        // user wants no more like this — Loop 2 hard suppression
    Ignored,         // expired without response (weak signal)
}
```

### 1.4 ContextSnapshot

The freeze-frame at commitment time. Stored separately from the
Commitment because it's heavy and we don't always need it loaded.

```rust
pub struct ContextSnapshot {
    pub id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub working_memory: Vec<TurnRef>,
    pub top_k_entities: Vec<(Uuid, f32)>,    // (entity_id, score) at decision time
    pub top_k_traces: Vec<Uuid>,             // recent traces
    pub topic_embedding: Vec<f32>,           // 384-dim
    pub ambient: AmbientState,
}

pub struct AmbientState {
    pub time_of_day: u8,
    pub day_of_week: u8,
    pub recent_capture_density: f32,     // captures per hour, last 4h
    pub recent_query_density: f32,
    pub source_app: Option<String>,
    // explicitly NOT collected: location, emotion, biometrics — out of scope
}
```

### 1.5 Storage layout

- `tm-intent` owns the types and state-machine validators.
- Persistence: SQLite tables in `~/.tracemind/memory.db`:
  - `commitments` (id, kind, statement, made_at, horizon, …)
  - `outcomes` (id, commitment_id, polarity, …)
  - `anticipations` (id, kind, confidence, user_response, …)
  - `context_snapshots` (id, captured_at, blob) — blob is bincode-serialized
- `tm-graph` adds typed edges: `Commitment ── derived_from ──► Commitment`,
  `Commitment ── about ──► Entity`, `Outcome ── evidenced_by ──► Trace`.

---

## 2. State machine

```
                  ┌──────────────────┐
                  │   (initial)      │
                  └────────┬─────────┘
                           │
                  capture / mine / MCP commit
                           │
                  ┌────────▼─────────┐
            ┌─────│       Open       │─────┐
            │     └────────┬─────────┘     │
            │              │               │
            │   user acted │   horizon     │
            │   (manually) │   prompt or   │
            │     or       │   implicit    │
            │   detected   │   match       │
            │              │               │
   user explicit           │              user explicit
   "abandoned"             │              "this is replaced by Y"
            │              │               │
            ▼              ▼               ▼
      ┌──────────┐   ┌──────────┐    ┌──────────────┐
      │Abandoned │   │  Acted   │    │  Superseded  │
      └──────────┘   └─────┬────┘    └──────────────┘
                           │
                outcome attached
                           │
                  ┌────────▼─────────┐
                  │    Completed     │
                  └──────────────────┘
```

Rules:

- **Open → Acted** is automatic when the system detects an action that
  matches the commitment statement (e.g. commitment "ship v2 Friday"
  + observed git tag `v2.0.0` on Friday). Without detection, the user
  can manually mark it.
- **Acted → Completed** requires an outcome record. Outcomes can land
  via brief prompt, voice "what happened", implicit text matching, or
  explicit MCP call.
- **Open → Superseded** when a new Commitment is filed with a `derived_from`
  pointer back. The pattern detector treats the chain as one trajectory.
- **Open → Abandoned** is user-initiated (or auto after 4× horizon
  with no signal of action — configurable).

---

## 3. Capture flow

Three intake paths. Friction goes from highest (manual) to lowest
(implicit mining), and we expect the volume distribution to invert
that — most captures are implicit.

### 3.1 Implicit phrase mining (target: 80% of captures)

A new `tm-capture::CommitmentMiner` watches existing capture streams
(clipboard, shell, MCP turns, browser highlights, voice transcripts)
for commitment-shaped phrases. Phrase set:

- Forward (intent): "I'll", "I'm going to", "let's", "the plan is", "I intend to", "I'll try", "going with", "decided to", "we'll ship", "I'm gonna"
- Backward (decision-disclosed): "I decided", "I went with", "ended up", "chose", "settled on", "we're doing"
- Hypothesis: "I think", "I bet", "my guess", "probably", "should be"

Each match becomes a **candidate Commitment** queued for confirmation:

- Surfaced in the next daily brief as: *"3 things you said yesterday — confirm any?"*
- One-tap (or one-key in CLI) to accept / edit / dismiss.
- Dismissed candidates do not retrain the miner unless the user
  silences the phrase pattern (rare).

The miner uses small LLM (Tier-1) at confirmation time to rewrite
the candidate into a clean statement, propose a horizon, and tag it.
At mining time it's a regex + heuristic gate (cheap, runs on every
capture).

### 3.2 Voice quick-capture (target: 15% of captures)

`tm-voice` already plans Whisper-tiny STT. New voice mode:

- Hotkey ⌘⇧Space → Whisper-tiny captures audio.
- Tier-1 normalizes to a Commitment draft.
- One confirmation tap.

Founder-natural phrases like "Hey, I'm gonna give vendor A two weeks
to fix the integration; if not we switch to B" → captured as an
Intent with horizon `made_at + 14d`, options ["vendor A fix", "vendor B"].

### 3.3 Explicit MCP / CLI (target: 5% but high signal)

```jsonrpc
{ "method": "memory_commit",
  "params": {
    "kind": "decision",
    "statement": "Use Postgres over Mongo for v2",
    "options": ["Postgres", "Mongo", "DynamoDB"],
    "expected_outcome": "scales to 100k events/day with ACID for billing",
    "horizon": "2026-08-01T00:00:00Z",
    "stakes": "high",
    "tags": ["infra", "v2"]
  }
}
```

CLI mirror:
```
tracemind commit \
    --kind decision \
    --statement "Use Postgres for v2" \
    --options Postgres,Mongo,DynamoDB \
    --expected "scales to 100k/day with ACID" \
    --by 2026-08-01 \
    --stakes high
```

LLM agents (Claude Code, Cursor) get a doc prompt: *"When the user
makes a decision in conversation, call `memory_commit` to record it."*
This is the cleanest signal source.

---

## 4. Outcome attachment

Three intake paths, mirroring capture.

### 4.1 Time-triggered prompt (default)

When a Commitment's `horizon` passes, `tm-reflect` queues a brief
prompt: *"On 2026-04-22 you committed to ship v2 by Friday. What
happened?"*

UX: three big buttons (👍 / 👎 / 🤷) + optional voice or text note.
Single tap = full outcome record. No outcome required — `NoOutcome`
is a valid Polarity for things that fizzled.

### 4.2 Implicit text matching

When new captures contain text whose embedding has high cosine
similarity (≥ 0.7) to an open Commitment's statement OR
expected_outcome, propose an outcome:

- "ended up missing Friday" + open commitment "ship v2 Friday" →
  proposed Outcome with `polarity: Worse`, `description` = matched text.
- Surfaced in next brief for one-tap confirm.
- If user confirms → `OutcomeSource::ImplicitMatched`. If user edits
  the polarity, the matcher retrains its threshold (Loop 2 signal).

### 4.3 Explicit MCP / CLI

```jsonrpc
{ "method": "memory_resolve",
  "params": {
    "commitment_id": "...",
    "polarity": "worse",
    "description": "shipped Tuesday, 4 days late, no critical issues",
    "evidence": ["trace:abc", "trace:def"]
  }
}
```

CLI: `tracemind resolve <id> --polarity worse --note "..."`.

---

## 5. Pattern detector (the meta layer)

Lives in `tm-reflect::PatternDetector`. Runs nightly. Outputs
human-readable patterns into the Anticipation pool with
`AnticipationKind::PatternMatch`.

### 5.1 Algorithm

For all `Completed` Commitments in the last 12 months:

1. **Bucket by feature cell.** A "cell" is a tuple of:
   - tag set intersection (top-3 most-frequent tags shared)
   - stakes class
   - source app cluster (e.g. "browser-research" vs. "shell-deploy")
   - time-of-day band (morning / afternoon / evening / night)
   - context-topic centroid bucket (k-means cluster of topic
     embeddings, k = 16 globally per user)
2. **Compute per-cell stats:**
   - `n` = count of completed commitments in cell
   - `polarity_dist` = (p_better, p_as_expected, p_worse, p_mixed)
   - `lift` = `p_worse(cell) - p_worse(global)`  (positive = worse than usual)
   - `support_strength` = wilson-lower-bound at 95% CI on `n`
3. **Surface pattern only if all true:**
   - `n ≥ 6` (minimum support — under-surface rather than confabulate)
   - `|lift| ≥ 0.25`
   - `support_strength` consistent with lift sign
   - user has not silenced this cell-shape in the last 90 days
   - no equivalent pattern starred / dismissed in last 30 days
4. **Render the pattern** with a fixed template (no model
   generation — too high a hallucination risk for this surface):

   > *"In the last {n} commitments tagged {tags} with stakes {stakes},
   > {p_worse}% had outcomes worse than expected — vs. {global_p_worse}%
   > overall."*

   Underneath: list the supporting commitments + outcomes, all
   clickable.

### 5.2 What we deliberately do NOT do

- No prose like "you tend to overestimate." That's an interpretation;
  the user gets to make it.
- No causal claims ("because you decide late at night"). Correlation
  surfaced; causation is the user's to draw.
- No psychometric framing ("you have a planning fallacy"). We are
  not a therapist. We are a logbook with statistics.

### 5.3 Calibration view

A settings panel exposes the detector's own track record:

- For each surfaced pattern, was it accepted / dismissed / silenced?
- For predictions made (Anticipations resolved later) — what fraction
  matched eventual_commitment within 7d?
- Last-30-day accuracy band. If accuracy < 50%, the surface auto-quiets.

The user can always see the pattern detector's report card. This is
the trust mechanism.

---

## 6. The three predictive layers

The core wedge upgrade. The world model emits Anticipations
continuously; only the *output channel* differs by layer.

### 6.1 L1 — Prefetch (silent, always on)

**Trigger:** any time the user starts an interaction (opens brief,
hits hotkey, starts dictating, types into the agent).

**Mechanism:** world model predicts top-3 likely topic centroids for
the next 30 seconds. `tm-retrieval` runs queries for those centroids
in the background; results cached. When the user actually asks, hit
rate is high → results feel instant.

**User-visible:** nothing — just lower latency. Anticipation record
keeps the score for calibration.

**Failure mode:** wasted work. Bound by a 50ms compute budget per
prediction; on cold CPU we skip.

### 6.2 L2 — Pattern surfacing (visible, opt-in)

**Trigger:** world model + pattern detector recognize current context
matches a prior commitment cell with surfacable pattern.

**Mechanism:** generate `AnticipationKind::PatternMatch`. Surface in:
- daily brief ("today's context looks like X — last 3 times you …")
- proactive notification (Tauri toast)
- sidebar in the agent

**User-visible:** structured surface, factual tone, citations
mandatory, dismiss/star/silence available.

**Default:** OFF for the first 30 days of use. After 30 days (or 50
captured commitments, whichever first) the brief asks: *"Want
TraceMind to start surfacing patterns when it spots them?"*

**Failure mode:** wrong / annoying patterns. Loop 2 (preference)
trains on user_response. Three silenced cells in a row → soft pause
the surface for a week, ask the user.

### 6.3 L3 — Recommendation (conversational, explicit opt-in)

**Trigger:** user asks. *"What would you do?"*, *"Help me decide"*,
or new MCP method `memory_recommend`.

**Mechanism:** for each option in current commitment-being-formed,
roll out with the world model conditional on (statement, context,
chosen=option) and aggregate predicted polarity. Return as
`AnticipationKind::Recommendation`:

> *"Given 14 prior commitments in similar contexts:
> Option A: 71% better/as-expected, 29% worse — closest match: <id>
> Option B: 40% better/as-expected, 60% worse — closest match: <id>
> Worth considering: <pattern surfaced from §5>."*

Always with sources. Never with imperatives ("you should X"). Always
ends with: *"You have the call."*

**Default:** disabled. User must enable in settings. Even when
enabled, requires explicit invocation per session.

**Failure mode:** confidently wrong recommendation in a high-stakes
spot. Mitigations: world-model accuracy gate (don't ship until
held-out accuracy > 60% on user's own commitments), confidence
intervals on every number, "show your work" expansion.

### 6.4 The gating ladder

```
Day 1                Day 30 (or 50 commitments)         User opts in
  │                          │                                │
  ▼                          ▼                                ▼
┌─────┐                ┌─────────┐                      ┌──────────┐
│ L1  │ ──────────────►│  L2     │ ────────────────────►│   L3     │
│ on  │                │ unlocks │                      │ unlocks  │
└─────┘                └─────────┘                      └──────────┘
silent                  visible                          conversational

each layer is independently togglable; no layer is forced
```

---

## 7. World model — training contract

The world model is the engine behind all three predictive layers.
This section is the *training contract*: what it predicts, what it's
trained on, how we evaluate it, and what we don't do.

### 7.1 What it predicts

Two functions in embedding space (≤ 384 dims):

```
f_topic : (working_memory_embed, ambient_state) → next_topic_embed
f_outcome : (statement_embed, context_snapshot_embed, chosen_option_embed)
              → outcome_polarity_logits  ∈ R^4
```

`f_topic` powers L1 (prefetch). `f_outcome` powers L2 (pattern
surfacing) and L3 (recommendation).

**Out of scope:** token-space generation about the user's life. The
world model never *writes* about the user. It produces embeddings
and logits. Token output (if any) comes from `tm-narrate` rendering
*stored* commitments + outcomes.

### 7.2 Architecture

- `f_topic`: 2-layer MLP, 384 → 512 → 384. Trained with contrastive
  InfoNCE loss; positives are the actual next-topic embedding,
  negatives are random recent topics. ~600k params.
- `f_outcome`: small transformer (4 layers, d=256, 4 heads) over
  concatenated (statement, context, option) tokens via the existing
  BGE tokenizer. Output head: 4-class (Better/AsExpected/Worse/Mixed)
  + a confidence scalar. ~3M params.

Both run on CPU. ~50ms per inference. Trained nightly in `tm-reflect`
with budget capped at 60 seconds.

### 7.3 Training data

- Per-user only. The model lives at `~/.tracemind/world_model.bin`,
  is never uploaded, and resets to a fresh prior on a single command.
- `f_topic`: triples `(history_window_embed, ambient, target_topic_embed)`
  drawn from the user's trajectory store.
- `f_outcome`: triples `(statement_embed, context_embed, option_embed) →
  polarity` drawn from `Completed` Commitments only.
- Cold-start prior: warm initialization from a small public-domain
  decision corpus (we can ship a tiny pretrained checkpoint that's
  already learned generic linguistic priors but knows nothing
  user-specific). The prior decays as personal data accumulates.

### 7.4 Evaluation

The model has its own LoCoMo-shaped local eval:

- Hold out the last 10% of resolved Commitments per user.
- Score `f_outcome` on held-out polarity prediction (4-class accuracy +
  Brier score for calibration).
- Score `f_topic` on next-topic recall@5 in held-out trajectory windows.
- Both numbers exposed in the calibration panel (§5.3).

**Layer gates:**
- L2 surfacing is allowed only when `f_outcome` Brier ≤ 0.30 on
  held-out (= the model is at least better-than-trivial on this user).
- L3 recommendations allowed only when `f_outcome` 4-class accuracy
  ≥ 60% on held-out *and* user has explicitly opted in.

### 7.5 Privacy + safety hard rules

1. **Per-user, local.** Never uploaded. Never shared. Never trained
   on aggregate cross-user data.
2. **Single-command reset.** `tracemind brain reset` wipes the model,
   anticipations, calibration history. The Commitments themselves
   are not deleted (separate command).
3. **Inspectable.** The model's prediction for any Anticipation can
   be queried via `tracemind explain <anticipation_id>` — returns
   the grounding Commitments, the cosine distances, the logits.
4. **Embedding-space only.** The model never emits text about the
   user. Any text the user sees comes from rendering existing
   Commitments + Outcomes through `tm-narrate`.
5. **Calibration-gated surfaces.** L2 and L3 auto-quiet when accuracy
   degrades.

---

## 8. MCP tool schemas

Three new MCP tools. Naming kept consistent with existing
`memory_store` / `memory_query`.

### 8.1 `memory_commit`

```jsonschema
{
  "type": "object",
  "required": ["kind", "statement"],
  "properties": {
    "kind": { "enum": ["intent", "decision", "hypothesis"] },
    "statement": { "type": "string" },
    "options": { "type": "array", "items": {"type": "string"} },
    "chosen": { "type": "string" },
    "expected_outcome": { "type": "string" },
    "horizon": { "type": "string", "format": "date-time" },
    "stakes": { "enum": ["low", "medium", "high", "reversible"] },
    "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
    "tags": { "type": "array", "items": {"type": "string"} },
    "derived_from": { "type": "array", "items": {"type": "string"} }
  }
}
```

Returns: `{ "commitment_id": "...", "context_snapshot_id": "..." }`.

### 8.2 `memory_resolve`

```jsonschema
{
  "type": "object",
  "required": ["commitment_id", "polarity"],
  "properties": {
    "commitment_id": { "type": "string" },
    "polarity": { "enum": ["better", "as_expected", "worse", "mixed", "no_outcome"] },
    "description": { "type": "string" },
    "evidence": { "type": "array", "items": {"type": "string"} },
    "user_note": { "type": "string" }
  }
}
```

Returns: `{ "outcome_id": "...", "commitment_state": "completed" }`.

### 8.3 `memory_recommend`

```jsonschema
{
  "type": "object",
  "required": ["statement"],
  "properties": {
    "statement": { "type": "string" },
    "options": { "type": "array", "items": {"type": "string"} },
    "context_hint": { "type": "string" },
    "max_references": { "type": "integer", "default": 8 }
  }
}
```

Returns:
```json
{
  "recommendation": null,                 // never auto-fills with an opinion
  "option_analyses": [
    {
      "option": "Postgres",
      "predicted_polarity_dist": [0.71, 0.21, 0.05, 0.03],
      "confidence": 0.62,
      "closest_prior_commitments": ["...", "..."],
      "patterns": [{"id": "...", "summary": "..."}]
    }
  ],
  "calibration_note": "model accuracy on your last 30 days: 67%",
  "reminder": "this is your call — TraceMind only shows what your past data implies"
}
```

---

## 9. UX surfaces

### 9.1 Daily brief

```
GOOD MORNING, {NAME}.    Wed Apr 28, 7:14 AM

  ▸ open intents (3)
      • ship v2 by Fri Apr 30 — 2 days left
      • call mom this weekend — soft horizon Sun
      • migrate to Postgres — horizon Aug 1, no progress signal

  ▸ resolved yesterday (1)
      • "give vendor A two weeks" → Completed (Worse): you switched 4 days early.

  ▸ patterns (1)                                    [tap to view]
      In your last 7 commitments tagged "vendor", outcomes
      worse than expected occurred 5 times.
      [show commitments]   [⭐ valuable]   [dismiss]   [silence]

  ▸ candidates from yesterday (3)               [implicit-mined]
      • "I'll handle the contract today"        [confirm | edit | skip]
      • "going with the second design"          [confirm | edit | skip]
      • "we should probably ship Tuesday"       [confirm | edit | skip]
```

### 9.2 Commitment timeline (Tauri)

A vertical timeline with one row per Commitment, color-coded by
state (Open=gray, Acted=blue, Completed-Better=green,
Completed-Worse=red, Abandoned=faded). Click → drawer with full
context snapshot, related commitments, outcomes.

### 9.3 Calibration panel (Settings)

```
Your TraceMind world model — last 30 days
─────────────────────────────────────────
Predictions made:                  142
  Auto-prefetch (silent):          120
  Patterns surfaced:                18
  Recommendations on request:        4

Outcome accuracy (held-out):       67%
Brier score:                       0.24    [green]
Topic recall@5:                    0.81

Patterns
  surfaced:                         18
  starred by you:                    4
  dismissed:                         9
  silenced:                          2

[reset world model]   [export commitments]   [open audit log]
```

### 9.4 The "show your work" expansion

Every Anticipation card has a "show your work" button. Tap → reveals:
- which Commitments grounded the prediction
- their outcomes
- the cosine distances driving the match
- the model's confidence + calibration on this *kind* of prediction

This is the trust contract. If we ever ship an Anticipation without
this button working, we've broken our own product promise.

---

## 10. Sprint plan (replaces §5 of `BRAIN_ARCHITECTURE.md`)

Seven sprints. Each sprint's success metric is now framed in terms
of the system-of-intents wedge, not generic memory quality.

### Sprint A — Real answers + first capture moat *(unchanged)*
Tier-1 LLM wired (`tm-answer` → `llama-cpp-2`), `NarrativeResponse`
default, Obsidian vault import, MCP structured ingest, LoCoMo ≥ 60.
**Felt:** "answers stop being a database."

### Sprint B — Commitment primitive + L1 prefetch
- New crate `tm-intent` (data model, state machine).
- `memory_commit` / `memory_resolve` MCP tools.
- `tm-capture::CommitmentMiner` (phrase mining) on existing streams.
- `tm-voice` — Whisper-tiny + Piper for ⌘⇧Space voice capture.
- `tm-types::WorkingMemory` ring buffer.
- World model v0: `f_topic` MLP, contrastive training nightly.
- L1 prefetch wired into `tm-retrieval`.
- `tm-reflect` skeleton: brief generator + outcome-prompt scheduler.

**Felt:** "I can talk to it. It captured what I committed to. The
brief shows my open intents."

### Sprint C — Outcome attachment + pattern detector + multimodal capture
- Implicit text matcher for outcomes.
- Pattern detector with §5 rules + Wilson-lower-bound gate.
- `AnticipationKind::PatternMatch` surfacing in brief.
- Two-speed ingestion (TM-5.1-001), observation hierarchy
  (TM-5.1-003), contradiction detection (TM-5.1-005).
- Screenshot capture + VLM caption.
- Browser extension for highlight capture.
- Calibration panel v1 (read-only).

**Felt:** "It noticed the same thing went sideways three times."

### Sprint D — Reward circuit
- `tm-preference` crate (Loop 2 — preference embedding).
- Loop 4 counterfactual replay using f_outcome.
- Iterative retrieval (TM-5.2-006) + context budget (TM-5.1-006).
- Capacity-aware bandit features (TM-5.3-004).
- Pattern detector reads preference signal; surfaces silenced cells
  drop out.

**Felt:** "It's getting my taste. The brief is shorter and sharper
than week one."

### Sprint E — Visible surface + L3 conversational layer (gated)
- Tauri commitment timeline, intent dashboard, brief archive.
- Memory garden as secondary tab.
- Onboarding (sample data → meaningful brief in 60s).
- Global hotkey ⌘⇧M.
- `memory_recommend` MCP tool + L3 surface — *enabled only after
  world-model accuracy gate passes for the user*.
- Obsidian plugin (TS, separate repo).

**Felt:** "I can see my own track record at a glance. When I ask
'what would you do', it shows me what I did."

### Sprint F — Mobile + photos + sync
- UniFFI bindings → iOS (SwiftUI) + Android (Compose).
- Apple FoundationModels backend on iOS 26+.
- Photo ingest (EXIF + VLM caption).
- `tm-sync` crate — Automerge over user-chosen iCloud / Drive.
- Optional encrypted-cloud Tier (opt-in only) for low-RAM Android /
  browser.
- L1 prefetch on mobile.

**Felt:** "It follows me. Voice-captured intent on the train shows
in the laptop brief that night."

### Sprint G — World model v2 + scale
- `f_outcome` transformer replacing v0 MLP.
- JEPA-style surprise scoring (TM-6.0-001).
- Held-out evaluation in CI; calibration gates auto-quiet bad models.
- HNSW vector index (TM-5.3-007).
- Artifact pruning (TM-5.3-002), artifactless eval (TM-5.3-003).
- Mamba history compression (TM-6.0-002).

**Felt:** "It's predicting outcomes and it's right more than wrong."

---

## 11. Honest risks (carried forward + new)

1. **Cold-start trap (months 0–2).** No commitments yet → no patterns
   → L2 dark → product feels like a journal app. *Mitigation:* L1
   prefetch works from day 1; manual journaling is itself the value
   prop in months 0–2; onboarding ships with sample data so user can
   *see* what the brief looks like full.
2. **Wrong-prediction recovery.** Mitigation: visible calibration
   panel; auto-quiet on bad accuracy; dismiss/silence is empowering.
3. **Anthropomorphic creep.** Mitigation: tone guide is statistical,
   not personal. The system never says "I think" — it says "in N prior
   commitments, the rate was X%."
4. **Statistical pareidolia.** Mitigation: support gates
   (`n ≥ 6`, lift ≥ 0.25, Wilson-LB consistent). Under-surfacing is
   the safe failure.
5. **Outcome ambiguity.** Mitigation: `NoOutcome` is a legitimate
   polarity. Pattern detector excludes them. Vague commitments still
   accrue value as decision-journaling.
6. **Privacy stakes higher than memory.** Mitigation: world model is
   per-user, local-only, single-command resettable, never uploaded.
   This is exactly the moat — cloud-AI-memory cannot match it.
7. **Hard-stakes recommendation failure.** Mitigation: L3 only after
   accuracy gate. Always with confidence interval. Always ends *"you
   have the call."*
8. **Capture-flow friction.** Mitigation: 80/15/5 split between
   implicit-mined / voice / explicit. Most users never type a single
   commitment by hand.

---

## 12. What we're NOT building (deliberate)

- **No predictive UI claiming user state** ("you seem stressed"). Out
  of scope. We're a logbook with statistics, not a wellness app.
- **No social layer.** Single-user only through Phase 4. Multi-agent
  peer paradigm (TM-5.3-001) considered post-1.0.
- **No cross-user pattern learning.** Each user's world model is
  trained on their data only. We will never see aggregate decision
  patterns.
- **No financial/medical recommendation surface.** L3 explicitly
  disabled for Commitments tagged in those domains; surface as
  patterns only.
- **No autonomous action.** TraceMind never acts on a Commitment —
  doesn't send the email, doesn't push the deploy, doesn't book the
  meeting. It logs, surfaces, predicts, and gets out of the way.

---

## 13. Positioning (final form)

Old: *"local-only memory OS"* (developer-platform shaped — losing
race).

Recent: *"the second brain that talks back, notices patterns, and
lives entirely on your machine"* (companion shaped — directionally
right).

**Now (system of intents):**

> **TraceMind is a system of intents. It captures what you commit to,
> the context you had when you decided, and what actually played out —
> then anticipates the decisions you're about to make using only your
> own track record. Nothing leaves your machine. Ever.**

This is the line. It points at:
- a job (capture and learn from commitments)
- a wedge (decision-makers under uncertainty — founders first, then
  PMs, lawyers, researchers, journalers)
- a flywheel (more commitments → more outcomes → better predictions)
- a moat (only-local + per-user + auditable)
- a differentiator (cloud-AI-memory cannot do this — they don't see
  your life)

The product is not memory. The product is *learning from your own
track record*, in private, with citations.

---

## 14. Decisions log

Confirmed 2026-04-26 / 2026-04-27 in working session with Aaditya:

- Internal architecture stays brain-shaped; external metaphor open. ✅
- Wedge = solo founder for *defaults*; engine universal; three modes
  at 1.0. ✅
- Size cap dropped above Tier-0; per-platform tier table governs. ✅
- Optional encrypted-cloud Tier is opt-in only. ✅
- Multimodal critical on every platform; redistributed across early
  sprints. ✅
- World model — yes, narrowly scoped (embedding-space only, hard
  rules in §7.5). ✅
- **System of intents is the wedge. ✅**
- **Three predictive layers (L1/L2/L3) with explicit opt-in
  gating. ✅**

---

## 15. Next concrete moves

If approved:
1. Scaffold `tm-intent` crate with the §1 types + state machine.
2. Wire `memory_commit` / `memory_resolve` MCP tools.
3. Add the implicit miner stub to `tm-capture` (regex pass + brief
   surfacing of candidates).
4. Stub `tm-reflect` with the daily-brief generator + horizon prompts.
5. Treat this doc (§10) as the canonical roadmap for the seven-sprint
   shape; older roadmap files have been retired.

Sprint A is a precondition for everything in B onward. The order
matters; B's value depends on Tier-1 prose answers from A.
