# Phase 4 — From Memory OS to Companion: a delight reset

**Status**: ✅ **APPROVED & ACTIVE 2026-04-27** — canonical sprint plan now lives in `docs/INTENT_SYSTEM.md` §10 (seven sprints A–G).
**Date**: 2026-04-26 (delight reset) → 2026-04-27 (system-of-intents wedge confirmed)
**Author**: working session with Aaditya
**Predecessor**: `docs/PHASE3.md` (tiered answers — completed)
**Companion**: `docs/INTENT_SYSTEM.md` (the wedge spec — read this first)
**Successor**: TBD

---

## ⚠ Read order

1. **Canonical roadmap** → `docs/INTENT_SYSTEM.md` §10 (seven sprints, current).
2. This doc — kept for the *delight diagnosis* (§1) and *experience layers* (§3, §4) which still hold.
3. The §6 gantt below has been **superseded** by the seven-sprint plan; see §6′ for the absorbed mapping.

---

## 0. Why this doc exists

After landing the LoCoMo harness and the v0.2 synthesizer fixes (F1
12.78 → 25.70), Aaditya said the product feels **flat and low quality**.
The model is weak (Tier 0 is barely 25 F1) and the product reads like
a database with a CLI on top. That's a fair read. This doc is a
brutal-honest reset of what TraceMind needs to be in order to
*matter* — and the architectural changes that follow from that reset.

The premise: **shipping a 25-F1 extractive memory CLI with a Tauri
wrapper is not a product. It's a research artifact.** The published
competitors (Mem0, Honcho, Letta, Zep, SuperLocalMemory) all clear
70+ F1 because they all use cloud or local LLMs as their primary
synthesis path. Tier 0 was never going to be the user-facing surface;
we just acted like it could be.

## 1. The honest diagnosis

### 1.1 Why it feels flat

1. **Generic category, no opinionated wedge.** "Local-only memory OS"
   is shared positioning with at least five other systems. None of
   them are loved by users; they're all loved by *developers shipping
   AI apps*. We've been competing in the wrong category.
2. **The user-facing surface is a CLI returning ranked entities.** The
   delight in memory products is *being told something you forgot*,
   *being shown a connection you missed*, or *getting a story about
   your own life*. We return triples and signal hits.
3. **No personality, no rituals, no daily loop.** Compare:
   - Apple Watch — closes rings every day → habit
   - Strava — kudos + segments → social loop
   - Duolingo — streaks + owl → guilt loop
   - **TraceMind today** — open terminal, type query, read results
4. **Privacy-first is defense, not offense.** "We don't send your
   data to the cloud" is a feature for paranoid people. It's not a
   reason for normal humans to install something. We need an
   *offensive* reason to exist.
5. **The math is against us at Tier 0.** Real LoCoMo on Tier 0 will
   land around 30–50 F1 even with all the planned synth fixes. Mem0
   is at 91.6. Even if a user doesn't know F1, they'll *feel* the
   60-point gap when their answers are wrong half the time.
6. **No "wow" in the first 60 seconds.** Install → ingest → query is
   an empty memory experience. Rewind shows you instantly. Notion AI
   writes you a draft instantly. We make people do data entry.

### 1.2 What's actually working

Don't throw the baby out:

- **The architecture is solid.** 17 well-factored crates, clean
  layer separation, real ONNX embeddings, audit-trail-by-default,
  bandit-driven retrieval. This is a *better foundation* than most of
  the competition.
- **Local-first is real.** The infrastructure is genuinely there —
  models bundled, no network in the request path, SQLite-only.
- **The capture surface is broad.** Clipboard + shell history + MCP +
  CLI ingest gives us multi-modal capture that beats Mem0's
  "POST to API" model on convenience.
- **The reasoning + analogy + causal-trace stack is differentiated.**
  Nobody else exposes a "why did you retrieve this?" narrative. That
  is a wedge.

So we keep the engine. We replace the *experience* and pivot the
*positioning*.

## 2. The pivot: from "memory OS" to "memory companion"

Stop calling it a memory OS. That phrase invites comparison to
databases. Reposition as:

> **TraceMind is the second brain that talks back, notices patterns,
> and lives entirely on your machine.**

The four pillars of this repositioning:

1. **It talks back** — voice in, voice out. Whisper local + Piper TTS.
   Memory you can talk to is qualitatively different from memory you
   query. This is the iPhone-vs-PalmPilot move.
2. **It notices patterns** — proactive insight loop. A daily reflection
   pass that surfaces connections, contradictions, loose ends, and
   patterns *you didn't ask for*. This is the offensive privacy move:
   *only* a local system can do this without leaking your life to
   OpenAI.
3. **It has a voice (personality)** — terse, slightly poetic, useful.
   Configurable. Default voice has a name and an aesthetic.
4. **It earns trust through transparency** — every answer cites
   traces. Every retrieved fact links to its origin. The graph is
   visualizable. We make local-first *visible*, not invisible.

### 2.1 Vertical wedge

We can't be all things. Pick one beachhead persona to design for.
Three candidates, in order of fit:

| Persona              | Why fit                                              | Risk                       |
|----------------------|------------------------------------------------------|----------------------------|
| **Solo founders**    | Already capture obsessively, value privacy, will pay | Small TAM, high churn risk |
| **Researchers/PhDs** | Think in long arcs, love citation, hate cloud risk   | Slow sales cycle           |
| **Therapy-adjacent** | Want to journal + reflect; privacy is non-negotiable | Regulatory minefield       |

Recommend **solo founders / indie operators** — that's also Aaditya's
own profile, the dogfood loop is tight, and Honcho/Mem0 are
developer-platform-shaped (not founder-shaped). The wedge:
*"the only memory system that's yours, that talks back, and that
notices what you missed this week."*

## 3. Delight loops (the product surface)

These are concrete user-visible experiences. Each is achievable
on this codebase with the architecture changes in §4.

### 3.1 The Daily Brief
Every morning at the user's chosen time, TraceMind generates a 60-second
synthesis:
- "Yesterday you talked about: <3 entity clusters>"
- "Loose end from 3 weeks ago: <unresolved item with citation>"
- "Pattern I'm noticing: <surfaced from analogies>"
- "Today you said you'd: <commitment from yesterday's capture>"

Delivered as: notification → opens Tauri panel → can be read aloud
(Piper TTS).

### 3.2 Surprise & Delight surfacing
Background reflection pass spots things like:
- "You've mentioned 'Stripe payment issue' three times this month."
- "You wrote about marathon training a year ago. Want me to surface it
  for Ethan's question?"
- "Three weeks ago you decided X. You just said something that
  contradicts that. Want to revisit?"

This is the offensive privacy moat: nobody else is allowed to do this
to you, because nobody else has all your captures.

### 3.3 Time-machine queries
First-class, not a side effect.
- "What was I worried about three months ago?" → narrative answer
  with citations + emotional arc.
- "Show me the moment I decided to leave Stripe" → sequence of turns
  with timeline visualization.
- "Replay last week" → daily-brief style summary of past week.

### 3.4 Voice as primary interface
- Hotkey (⌘⇧Space) → voice-mode overlay → speak query → answer back
  in 1–2 sentences via Piper.
- Local Whisper-tiny (39MB) for STT.
- Piper or coqui-ai for TTS, ~30MB.
- Latency budget: ≤2s end-to-end on M-series.

### 3.5 Memory garden (graph viz)
The graph is the product, surface it. Tauri view:
- Force-directed view of entities + edges, colored by recency/heat.
- Click an entity → see all turns it appears in, all reasoning
  chains, all analogies.
- "Why did you connect X and Y?" → causal-trace narrative.

### 3.6 Daily ritual: the 60-second capture
Push notification at end of day: "30 seconds — what mattered today?"
→ voice-capture → ingested. Builds the streak. Builds the loop.

## 4. Architecture changes to support delight

These are *new* layers and crates. The existing 17 stand.

### 4.1 New crate: `tm-narrate` (Phase 4)
Generative narrative composer. Given a set of retrieved chunks,
produces *prose* — a 1–4 sentence synthesis with citations, in the
configured voice. Used by Daily Brief, surprise surfacing, time-machine
queries. Always Tier-1+ (no extractive narrative).

Depends on: `tm-answer`, `tm-retrieval`, `tm-types`. Requires
`local-llm` feature on `tm-answer`.

### 4.2 New crate: `tm-reflect` (Phase 4)
Background reflection daemon. Runs:
- nightly consolidation (already in `tm-reason::Consolidator`)
- pattern detection (clusters of related entities, contradictions
  via `AnalogySolver` + Tier-1)
- daily-brief generator (queues for morning delivery)
- "loose end" detector (entities with high recent activity but no
  resolution edge)

Depends on: `tm-retrieval`, `tm-reason`, `tm-narrate`, `tm-types`.

### 4.3 New crate: `tm-voice` (Phase 4)
Local Whisper STT + Piper TTS. Streaming microphone capture, push-to-talk
hotkey, voice playback. Feature-gated — not all users want voice.

Models: `whisper-tiny` (39MB), `piper-en-amy-medium` (60MB).
Depends on: `tm-types`, `tm-capture`.

### 4.4 New crate: `tm-companion` (Phase 4)
Personality / voice / system prompt configuration. Stores the
user-configurable persona (name, voice tone, brief style). Used by
`tm-narrate` and `tm-voice`. Trivially small — config + a few prompts.

### 4.5 New layer: working memory (in-RAM)
Short-term in-process cache for the current session — last N turns,
recent retrievals, current emotional/topic state. Lets us answer
"what did we just discuss?" without DB hits and gives the system a
sense of conversational *continuity* (the missing piece in current
TraceMind).

Implementation: `tm-types::WorkingMemory` (Vec-backed ring buffer of
recent turns + retrieval results). Wired through `tm-mcp`, CLI, Tauri.

### 4.6 Tier 0 demotion + Tier 1 promotion
**Make Tier-1 the default user-facing path.** Tier 0 stays as the
"degraded fallback" but the install flow downloads Tier-1 weights by
default unless the user explicitly opts out. The 900MB download
happens during onboarding while we're showing the value-prop video.

This single change probably moves felt-quality from 25 F1 to 60+ F1
on real workloads. The product can't be delightful at Tier 0; pretend
otherwise wastes our time.

### 4.7 Continuous user-facing eval
Drop SQuAD-F1-as-north-star. Real product metric:
- After every answer, optional thumbs-up/down + comment.
- Per-week aggregate: "answers you found useful" / "answers you
  corrected" / "questions you re-asked".
- Local-only (we never see it). Persisted to
  `~/.tracemind/feedback.jsonl`. Can be exported for self-reflection.

LoCoMo stays as the *regression gate* — but it's not the product
metric. The product metric is *did the user feel understood*.

### 4.8 Visualization layer (`tm-tauri` work)
The Tauri app gets:
- Daily brief panel (read on open, dismissable, archived)
- Memory garden graph view
- Voice mode overlay
- Capture timeline (chronological view of recent ingests)
- "What I noticed" surprise panel
- Settings for personality/voice/brief schedule

This is the bulk of the polish work. The Rust core is mostly done;
the experience layer is the gap.

### 4.9 Multimodal capture (Phase 4.5)
- Screenshot ingestion via `tm-capture` + a small VLM (Phi-3.5-vision
  or Moondream) for caption-then-store.
- Voice notes ingested via `tm-voice`.
- Photos: EXIF + caption.

This is the move that makes memory *actually feel like memory* — your
brain doesn't store text, it stores moments.

### 4.10 The narrative answer contract
Every user-facing answer (CLI, MCP, Tauri) becomes a `NarrativeResponse`:
```
{
  text: "...",                    // 1-4 sentences, Tier-1+
  citations: [Citation],          // grounded back to traces
  related_threads: [Thread],      // 1-hop adjacencies the user might want
  surprise: Option<Insight>,      // proactive observation if relevant
  voice_audio: Option<Bytes>,     // pre-generated TTS if voice mode
}
```

`NarrativeResponse` replaces today's raw `RetrievalResult` at the
boundary. `RetrievalResult` becomes an internal type.

## 5. Quality bar shifts

### 5.1 Stop optimizing for SQuAD F1
- Keep LoCoMo as a regression gate (it catches us breaking things).
- Add a smaller, in-house eval set of "founder-day" queries scored
  on a 1–5 helpfulness rubric (LLM-judge using Tier-1, optional).
- Ship the user-feedback loop in §4.7.

### 5.2 Three quality bars
- **Floor**: LoCoMo F1 ≥ 60 (must clear or block release).
- **Bar**: Daily-brief generation succeeds on a held-out test corpus
  with > 80% citation correctness.
- **North star**: Weekly user-survey median ≥ 4/5 on
  "the brief noticed something useful this week".

### 5.3 Latency bars
- Cold query (Tier-0): ≤ 200ms.
- Hot query (Tier-1, 1.5B model): ≤ 1.5s on M-series.
- Voice end-to-end: ≤ 2s.
- Daily brief generation: ≤ 30s background.

### 5.4 Footprint bars (revised)
Old: `<200MB idle / <500MB active`. New, honest:
- Tier-0 only: `<200MB idle, <500MB active`. (Already met.)
- Tier-1 enabled: `<400MB idle (model unloaded), <1.6GB active`.
- Full companion (Tier-1 + voice): `<1.8GB active`.

Above 2GB on a MacBook Air is fine. Below 200MB idle is
non-negotiable — that's the "you don't notice it's running" promise.

## 6. The 6-month gantt (SUPERSEDED — see §6′ below)

The 6-sprint delight gantt below was the April-26 plan, **before** the
April-27 wedge pivot to a system-of-intents. It is preserved for
historical reference. Use §6′ for current planning.

| Sprint | Theme                          | Concrete deliverables                                                         |
|--------|--------------------------------|-------------------------------------------------------------------------------|
| 1      | **Tier-1 actually works**      | Wire `tm-answer::LocalLlmBackend` to llama-cpp-2; bundle Qwen 2.5 1.5B; default-on download in installer; new LoCoMo run with Tier-1 — target F1 ≥ 60 mini-set / ≥ 50 full |
| 2      | **NarrativeResponse**          | Every CLI/MCP answer becomes prose with citations; deprecate raw triple lists in user-facing paths |
| 3      | **Working memory + Daily brief** | `tm-types::WorkingMemory`; nightly `tm-reflect` cron; brief renders in Tauri; user can dismiss/star |
| 4      | **Voice mode**                 | `tm-voice` crate; Whisper-tiny + Piper bundled; ⌘⇧Space hotkey; voice replies on demand |
| 5      | **Memory garden + onboarding** | Tauri graph view; first-run experience that ingests sample data and shows the brief in 60s |
| 6      | **Surprise loop + multimodal** | `tm-reflect` pattern detection; screenshot ingestion via VLM; "what I noticed this week" panel |

## 6′. Active sprint plan (canonical: INTENT_SYSTEM.md §10)

After the wedge pivot, the seven sprints A–G in `INTENT_SYSTEM.md` §10
are the canonical plan. The §6 themes are absorbed; only the *unit of
value* changes (commitment / outcome / anticipation), not the
underlying experience layers.

| Sprint | Theme (Intent System framing)                  | Carryover absorbed                            | Status |
|--------|------------------------------------------------|-----------------------------------------------|--------|
| **A**  | Real answers + first capture moat              | Phase 3 Tier-1 wiring, Obsidian vault import, MCP structured | ✅ structurally done; LoCoMo ≥60 number = tail item |
| **B**  | Commitment primitive + L1 prefetch             | §6 sprint 2 (NarrativeResponse), §6 sprint 3 (working memory), §6 sprint 4 (voice) — partial | 🚧 active |
| **C**  | Outcome attachment + pattern detector + multimodal capture | §6 sprint 6 (surprise + multimodal)  | pending |
| **D**  | Reward circuit (preference, counterfactual replay)  | iterative retrieval + context budget          | pending |
| **E**  | Visible surface + L3 conversational layer (gated)   | §6 sprint 5 (memory garden + onboarding); .dmg; Apple FM Tier-2; Obsidian plugin | pending |
| **F**  | Mobile + photos + sync                              | iOS / Android via UniFFI; tm-sync             | pending |
| **G**  | World model v2 + scale (LoCoMo ≥85, HNSW, Mamba)    | Phase 3 north-star; TM-5.3-007 HNSW           | pending |

After Sprint G, public beta launch on macOS via signed .dmg with
auto-update. Linux/Windows wait until product-market fit.

### Why this reorg

The pre-pivot plan optimized for *narrative quality* (Tier-1, brief,
voice). The post-pivot plan optimizes for *the unit that compounds*:
the Commitment. Narrative quality is now a downstream consequence —
once you have Commitments + Outcomes + Anticipations, the Daily Brief
writes itself. Voice and memory garden remain as *surfaces* on top of
the intent substrate, not as the substrate itself.

## 7. Things to *not* do

- **Don't ship a web/cloud version.** That kills the wedge.
- **Don't add multi-user / team features yet.** Solo-founder first.
- **Don't chase the LoCoMo leaderboard.** We'll never out-Mem0 Mem0
  on their own bench. Compete on lived experience.
- **Don't add a "TraceMind for X" plugin marketplace** until we have
  10k DAU. Plugins are a distraction tax for early-stage.
- **Don't mock the daily-brief schema with placeholders.** If the
  brief is generic ("Today you mentioned 3 entities"), users will
  uninstall in 2 days. Either it's specific and surprising, or it
  doesn't ship.

## 8. The asks

1. **Validate the repositioning** — do we agree this stops being a
   memory OS and starts being a memory companion? If yes, we update
   the website / README / pitch deck accordingly.
2. **Pick the wedge persona** — solo founder vs. researcher vs.
   journaler. I argued for solo founder. Push back if you disagree.
3. **Approve the Tier-1-default install** — this is the single
   biggest perceived-quality move. It costs us 900MB of download and
   ~1.2GB of RAM. I think it's worth it. Decide.
4. **Approve the Phase 4 scope** — six sprints, four new crates, a
   real Tauri UI build-out. This is roughly a quarter of work.
5. **Decide what we ship at end of Phase 3** — the answer should be
   *nothing public*. Phase 3 was infra. Phase 4 is the product.
   Don't launch a 25-F1 CLI to Hacker News. Wait.

## 9. Closing

The repo is good. The roadmap was incomplete. We were building a
research artifact and forgot to build a product around it. The fix
isn't to rebuild — it's to add the *experience* layers (`tm-narrate`,
`tm-reflect`, `tm-voice`, `tm-companion`) on top of the engine that
already exists, default Tier-1 on, and reposition around *companion*
not *OS*.

The boring memory-OS race is already lost to Mem0/Honcho. The
*companion* race hasn't started yet. We have the foundation. We need
the soul.
