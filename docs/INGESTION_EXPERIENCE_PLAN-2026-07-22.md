# Ingestion Experience + Safe Novel Data Collection Plan — 2026-07-22

**Framing:** the multi-modal plan (`PRODUCT_EXPERIENCE_PLAN.md` §3) covers **what** we ingest — screenshots, web, PDF, voice, email. This plan covers **how it feels** to feed data in, and the **novel primitives** that let us collect richer signal *without* the surveillance-tool aftertaste that kills local memory products.

Every existing consumer memory product (Rewind, Mem0, Personal.ai, Engram) hits the same wall: users install, get creeped out by ambient capture within 72 hours, uninstall. The wall is **not** a technical problem — it's an experience problem. Local + open-weights + on-device solves the *risk* but not the *feeling*. This plan is about the feeling.

**Companion to:** `PRODUCT_EXPERIENCE_PLAN.md` (surfaces), `F1_IMPROVEMENT_PLAN.md` (quality), `CHARTER-H2-2026.md` (strategy).

---

## 1. Honest diagnosis (why ingestion feels bad today)

**Capture is invisible.** `tm-capture` runs as a daemon; users don't see what was captured, when, or why. No audit surface. When they eventually look, they either find nothing useful (uninstall) or find something they didn't want captured (uninstall + one-star review).

**Consent is binary and up-front.** OS permissions dialog fires at install, user clicks "grant" because they want the product to work. Six weeks later they have no idea what they consented to. This is the *dark pattern* pattern — consent theater.

**Every capture is treated equally.** A stack trace from Xcode and a Signal message from a friend land in the same pipeline with the same weight. Governance PII gate is a coarse filter, not a semantic one.

**No user intent attached.** Ingestion has no purpose annotation — "why did I capture this?" is a question the user can't answer, so neither can retrieval.

**No review surface.** No "here's what got captured today, keep / refine / forget." Users can't triage what they don't see.

**Undo is per-item, not per-batch.** You can `tracemind purge-stopwords` but you can't say "forget everything from Tuesday afternoon between 2 and 4 pm" without SQL.

**Preprocessing is opaque.** NER extraction happens on write, silently. Users have no way to see "TraceMind thinks the entities in this clipboard event were: Foo, Bar, Baz — is that right?" before it's committed.

**Verdict:** the ingestion layer is engineered for *the pipeline*, not for *the person feeding it*.

---

## 2. North star

**The user should feel like they're collaborating with a librarian, not being watched by a wire-tap.**

Three concrete feelings by week 4:

1. **"I can see exactly what it knows about me"** — audit surface is one click from the Brief, chronological, filterable, per-source.
2. **"When I want it to pay attention, I tell it. When I want it to look away, I tell it."** — capture has explicit *intent modes* the user activates.
3. **"When it captured something wrong, undoing it took two clicks and the derived stuff went away too."** — reversibility with propagation, not just row deletion.

If any of those three doesn't land, ingestion is a liability.

---

## 3. Ingestion experience redesign (`tm-capture` + `tm-ingest` UX surfaces)

### 3.1 Three ingestion modes (the user chooses per-session, not per-source)

**Mode 1 — Ambient (default, low-signal).** Passive watchers run: clipboard (with smart filter), browser reading-time (Reader Mode dwell > 30s), user-picked file watchers. Everything lands in **quarantine tier** for 48h before promoting to hot.

**Mode 2 — Focus (user-initiated, high-signal).** User taps ⌥⌘F, names the focus session ("debugging build issue" / "call prep with Sarah" / "Q4 roadmap"). For the duration:
- Every capture is tagged with the focus intent
- Capture rate goes up (screenshot every 5min, not every 30min)
- Everything auto-promotes to hot at session end
- Session gets a Brief card summarizing what was captured

**Mode 3 — Private (user-initiated, zero-signal).** User taps ⌥⌘P. For the duration:
- All watchers pause
- Menu-bar icon changes color so it's visible
- Manual "capture this" still works, but timestamps are excluded from behavioral model
- Auto-resumes after a user-set duration or explicit toggle

These three modes make ingestion **a thing the user does**, not something that happens to them.

### 3.2 Quarantine → Hot → Warm → Cold (extends Q3.6 tiers)

**Quarantine (new tier before Hot):** 48h holding pen for ambient captures. User can review, keep, or discard in bulk. Auto-promotes to Hot at 48h if no action taken (user's implicit consent by not rejecting). Governance gate runs *twice* — once on write, once on promotion — so PII that snuck through the first gate can be caught.

**Existing tiers (Q3.6):** Hot / Warm / Cold with promote/demote per current implementation.

Quarantine is where the safety story lives. Every new source spends 48h there before it can influence anything.

### 3.3 Ingestion review surface — "Today in your memory"

New Tauri view: `IngestionReviewView.tsx`. Daily digest card in the Brief:

> **Today captured: 47 items across 4 sources.**
> · 23 from Chrome (technical reading)
> · 12 from clipboard (mostly commit messages)
> · 8 from screenshots (Xcode error dialogs)
> · 4 from your voice notes
>
> [Review · Keep All · Discard All]

Clicking Review opens a chronological timeline. Every item has:
- Preview
- Source + timestamp + trust level
- Why it was captured ("clipboard event, > 30 chars, non-sensitive app")
- Actions: Keep · Refine · Discard · Discard + Forget Everything Derived

**Refine** is the key novel action — the user edits the extracted representation before it enters the graph. If NER pulled the wrong entities, user fixes them once and TraceMind learns.

### 3.4 Consent receipts (per-capture, browsable)

Every capture writes a **receipt** to `~/.tracemind/receipts.jsonl`:

```json
{
  "capture_id": "…",
  "at": "2026-07-22T15:14:03Z",
  "source": "clipboard",
  "app_context": "Xcode",
  "modality": "text",
  "size_bytes": 412,
  "gate_decisions": {"pii_pass": true, "noise_pass": true, "sensitive_app": false},
  "tier_on_write": "quarantine",
  "user_intent": null,        // set if in Focus mode
  "why_captured": "clipboard >30 chars, non-sensitive app, matches active focus question"
}
```

Receipts are queryable in the Ingestion Review view. This is the audit surface. **No capture without a receipt.**

### 3.5 Refinement UX — the librarian metaphor

Instead of committing extraction on write, TraceMind can operate in **librarian mode** for high-value captures (Focus mode, user-tagged, or high-confidence-of-importance): the extracted entities/triples appear as a *proposal* the user glances at and confirms.

Not for every capture (would be exhausting). Just for the top-K per-day where getting extraction right compounds most. This is the same pattern as the ontology proposer (Q3 EVG-ONT), applied to individual entity extractions.

---

## 4. Novel data collection primitives — 20 ideas ranked

Ordered by (a) *how much richer the signal* vs (b) *how safe the primitive is by design*. Cheap and safe first, expensive or fragile later.

### Tier N1 — cheap, high consent, high signal

**N1.1 — Focus mode capture (§3.1 Mode 2).** User intent explicitly attached to every capture in the window. ~500 LOC. Best signal-to-noise upgrade in the whole plan.

**N1.2 — Question queue.** User types questions they can't answer yet into a persistent queue. TraceMind watches all future ingestion for signals matching a question. When it finds one, it Briefs the user: "You asked X last Thursday. This document seems relevant." ~400 LOC. Turns forgetting into a feature.

**N1.3 — Voice bookmarking.** User says "hey memory, remember this" while looking at anything. TraceMind captures screen state + last 5s of audio context + timestamp. No always-on mic — push-to-talk only. ~600 LOC.

**N1.4 — Whiteboard / paper capture.** One-tap photo → OCR (Vision framework) + auto-index. Explicitly user-initiated. Bridges physical → digital memory. ~300 LOC (piggybacks M1 screenshots).

**N1.5 — End-of-day journal.** 3-min voice or text prompt: "what did you work on, what's unresolved, what did you learn?" Highest-value single capture of the day; users control what goes in. ~400 LOC.

**N1.6 — Post-meeting debrief prompt.** Calendar integration (opt-in): after a meeting ends, menu-bar notification "add a 30s note about this meeting?" Never records the meeting itself — captures the reflection. ~500 LOC.

**N1.7 — Anti-goal rules.** User declares negative rules: "never capture from Signal / #random / Personal.app / URLs matching \*banking\*." Live-configurable, receipts show what was skipped so user can verify. ~200 LOC.

**N1.8 — Application scoping.** Per-app rules layered on top of ambient mode: "always capture from Xcode + VS Code, never from Messages, ask on first capture from any new app." First-capture-from-new-app is a Brief prompt, not silent. ~300 LOC.

### Tier N2 — medium cost, novel, still safe by design

**N2.1 — "Capture chain" bundles.** User taps one item ("start a chain"), taps N more within a session, taps end. The chain becomes a first-class memory unit that surfaces together. Explicit bundling replaces guessed clustering. ~400 LOC.

**N2.2 — Retro-capture.** User selects a past time window ("last Tuesday 2-4pm") and TraceMind re-processes what it captured with a new prompt/schema. No new data collected — just re-derivation. Powerful for "I'm now realizing this was important." ~500 LOC.

**N2.3 — Question-driven capture window.** Before starting a task, user states the intent question ("I'm figuring out our Q4 pricing"). Every capture in the next N minutes is tagged with that question and stored as evidence toward it. Related to N1.1 but more question-shaped. ~300 LOC.

**N2.4 — Ephemeral capture (24h auto-delete).** User taps "capture this ephemerally" — item stored for 24h, auto-deleted unless promoted. Reduces friction for capturing "just in case" without permanent commitment. ~250 LOC.

**N2.5 — Time-locked memories.** User marks a memory "don't surface until N months." Letter-to-future-self primitive. Uses existing `valid_from`/`valid_to` schema (Q3.4). ~200 LOC.

**N2.6 — "Watch for contradiction" opt-in.** User marks a memory as high-priority for contradiction detection. Boosts weight in Q3.4 conflict scan. Turns implicit contradiction into explicit interest. ~200 LOC.

**N2.7 — Reflective end-of-week review.** Sunday-morning Brief card: "here are 12 things you captured this week. Keep 8, refine 3, forget 1?" Batch triage instead of per-item friction. ~400 LOC.

**N2.8 — Physical anchor via NFC/QR.** User taps a physical NFC tag (or scans a QR code) — "everything I capture in the next hour is about the object this tag is on." Wedge-native for anyone with physical projects. ~350 LOC.

### Tier N3 — higher cost or higher trust burden

**N3.1 — Peer memory transfer.** Two devices with TraceMind, one taps to send a memory ("AirDrop for facts"). Never for personal data — for shared context in known-good relationships. ~800 LOC + trust design work. Rare surface but distinctive.

**N3.2 — Handwriting / notes-app watch.** Opt-in per-folder: Notes.app, Bear, Obsidian, iA Writer. Read-only sync of user-picked notebooks. ~500 LOC per integration.

**N3.3 — Email digest via forwarding address.** User forwards selected emails to `me@tracemind.local` (routed through local SMTP catch, no cloud). Ingestion by explicit forward, not IMAP scrape. ~700 LOC.

**N3.4 — Read-only OAuth (Notion / Obsidian sync / Roam).** Periodic pull with user-selected pages. Standard OAuth, minimum scope. ~1000 LOC per integration.

**N3.5 — GitHub / Linear activity watch.** Only user's own activity (their PRs, their issues, their comments). No org-wide scraping. ~800 LOC per integration.

**N3.6 — Calendar read-only.** EventKit / Google Calendar read-only. Meeting titles + attendees + user's own notes. Never the meeting content itself. ~500 LOC.

### Tier N4 — creative but off-wedge for H2 (documented for later)

- **Real-time meeting capture** — needs always-on ASR; wrong wedge
- **Video ingestion** — too heavy locally
- **Cloud-synced shared memory graphs** — off-wedge per positioning commitment
- **Passive location tracking as episodic anchor** — creepy > useful for our audience
- **Biometric / focus data (Apple Watch heart-rate as attention signal)** — too invasive for opt-in trust budget

---

## 5. Safety mechanisms — the ten commandments

Every capture primitive in §4 obeys all ten. If a mechanism violates any, it doesn't ship.

| # | Mechanism | What it guarantees |
|---|-----------|--------------------|
| S1 | **Receipt per capture** | Every write leaves an auditable record. No silent captures. |
| S2 | **Quarantine before hot** | 48h holding pen; PII gate runs twice. |
| S3 | **Reversibility with propagation** | Deleting a memory deletes its derived embeddings, triples, and reflections. Not just row-level. |
| S4 | **Per-source trust levels** | Ambient (medium) / User-typed (high) / Reflection (untrusted). Governance treats them differently. |
| S5 | **Sensitive-app blocklist by default** | Signal, Messages, 1Password, banking apps, health apps — capture disabled out of the box. User must opt-in per-app. |
| S6 | **Sensitive-tier requires OS auth** | Memories marked "sensitive" require Touch ID / password to retrieve. |
| S7 | **Ingestion is local-only, cryptographically** | Hash chain of ingestions signed by user's local key. Third-party auditable that nothing left the device. |
| S8 | **Purpose limitation** | Captures tagged with an intent (Focus mode, question queue) are used only within that intent's retrieval scope by default. |
| S9 | **Time-boxed captures default off** | Focus mode auto-ends at set duration. Ambient captures respect user's Private mode. No "left on forever" defaults. |
| S10 | **Attestable export + delete-all** | User can export their entire memory (Merkle-proof of completeness) and delete-all (with derivations) in ≤ 2 clicks. |

**Governance-layer additions to enforce these:**
- New `capture_gates.rs` in `tm-governance` — extends existing PII gate with per-app / per-source / per-intent policies
- New `CaptureReceipt` type in `tm-types` — surfaces the receipt as a first-class thing, not a log line
- New `PropagateDelete` trait — every crate that stores derived data from a memory implements it; `tm-graph`, `tm-vector`, `tm-episodic`, `tm-reflect`, `tm-controller` all wire in

---

## 6. Concrete goals with metrics

Every goal has a falsifiable metric measurable **without asking the user**.

| # | Goal | Metric | Deadline |
|---|------|-------:|:---------|
| I1 | **Receipt coverage** | 100% of captures produce a receipt row. Zero receipt-less writes in bench. | Week 4 |
| I2 | **Quarantine adoption** | ≥ 80% of ambient captures pass through quarantine tier before hot. | Week 4 |
| I3 | **Refine action usage** | ≥ 15% of high-value captures (Focus mode + user-tagged) get a Refine action within 24h. | Week 8 |
| I4 | **Sensitive-app blocklist correctness** | Zero captures from a blocklist-default app across 4-week install cohort. | Week 4 |
| I5 | **Focus mode adoption** | ≥ 40% of active users trigger Focus mode ≥ 1× per week by week 4. | Week 8 |
| I6 | **Question queue engagement** | ≥ 20% of active users have ≥ 1 open question by week 2. | Week 6 |
| I7 | **Discard-with-propagation completeness** | Deleting a memory reduces derived-row count by 100% in ≥ 99% of cases (integration test). | Week 6 |
| I8 | **Time-to-audit** | User can find any specific capture (given fuzzy description) in ≤ 3 clicks from Brief in ≥ 80% of trials. | Week 8 |
| I9 | **Consent-mode compliance** | Zero captures during any active Private mode window across bench + first 4-week install cohort. | Week 4 |
| I10 | **Cold-uninstall rate at 14 days** | ≤ 25% (industry norm for ambient-capture products is ~60%). | Week 12 |

**I10 is the single number** — if uninstall rate stays above competitor baseline, the safety story is unbelievable regardless of everything else.

---

## 7. Ordered todo list (I1–I18)

Ordering discipline: safety mechanisms first (nothing else ships until S1–S4 are enforced), then UX surfaces, then novel primitives. New primitives cost nothing if the surface can't display them.

### Sprint I-P1 (weeks 1–2) — safety foundation

- **I1 — Capture receipt system.** New `CaptureReceipt` type + `~/.tracemind/receipts.jsonl` + `tm-capture` writes receipt for every event + `tm-ingest` requires receipt to accept. ~500 LOC.
- **I2 — Quarantine tier.** Extends Q3.6 `MemoryTier` with `Quarantine` variant; 48h auto-promote + user-triggered promote/discard; second-pass PII gate on promotion. ~600 LOC.
- **I3 — Sensitive-app blocklist (default).** Bundled deny-list (Signal, Messages, 1Password, common banking/health apps); per-user extend/override in Settings. ~300 LOC.
- **I4 — Propagate-delete infrastructure.** New `PropagateDelete` trait; each derived-data crate implements it; `memory_forget` MCP verb calls the trait; integration test proves no orphans. ~700 LOC.

**End of I-P1:** every capture is receipted, quarantined, and reversible. Nothing novel yet; foundation only.

### Sprint I-P2 (weeks 3–4) — user-facing audit + review

- **I5 — Ingestion Review view (`IngestionReviewView.tsx`).** Daily digest card in Brief + full chronological view with filters + per-item actions (Keep/Refine/Discard). ~800 LOC UI + ~200 LOC backing.
- **I6 — Three-mode ingestion (Ambient / Focus / Private).** Mode selector in menu-bar; Focus mode session tracking; Private mode watcher-pause; menu-bar color/icon reflects mode. ~600 LOC.
- **I7 — Refine action wiring.** User edits extracted entities/triples; edits become a training signal for future extractions (feeds Q4.11 Curator). ~500 LOC.
- **I8 — Consent-mode compliance tests.** Bench harness that asserts I4/I9 across a synthetic capture stream. ~300 LOC.

**End of I-P2:** users can see what was captured, why, and undo it cleanly. Trust foundation shipped.

### Sprint I-P3 (weeks 5–6) — Tier N1 primitives

- **I9 — Focus mode capture (N1.1).** ~500 LOC.
- **I10 — Question queue (N1.2).** New MCP verb `question_pin`; watcher over ingestion for matches; Brief card when a match fires. ~400 LOC.
- **I11 — Voice bookmarking (N1.3).** Menu-bar push-to-talk; screen state capture on tap. ~600 LOC (piggybacks M4 voice from product plan).
- **I12 — Anti-goal rules (N1.7).** Live-configurable deny list; receipts show skipped captures. ~200 LOC.
- **I13 — Application scoping (N1.8).** Per-app rules + first-capture-from-new-app Brief prompt. ~300 LOC.

**End of I-P3:** the "librarian, not wire-tap" feeling should measurably land — I10 uninstall metric starts being trackable.

### Sprint I-P4 (weeks 7–8) — Tier N1 finish + Tier N2 novel

- **I14 — End-of-day journal (N1.5) + Post-meeting debrief (N1.6).** ~900 LOC total (calendar hook + prompt UX).
- **I15 — Capture chain bundles (N2.1) + Retro-capture (N2.2).** ~900 LOC together — both cheap once N1.1 focus infra exists.
- **I16 — Question-driven capture window (N2.3) + Ephemeral capture (N2.4).** ~550 LOC.
- **I17 — Time-locked + Contradiction opt-in (N2.5 + N2.6).** ~400 LOC.

### Sprint I-P5 (weeks 9–12) — polish + Tier N3 selective integrations

- **I18 — Reflective end-of-week review (N2.7) + physical NFC/QR anchor (N2.8).** Retention hooks.
- One Tier N3 integration: **Notes.app / Obsidian read-only sync** (highest-request likely). ~500 LOC.
- Attestable export + delete-all (S10) fully polished; Merkle-proof of completeness.

Tier N3.1 peer transfer, N3.3 email forwarding, N3.4 OAuth pulls, N3.5 GitHub/Linear, N3.6 calendar → all deferred to post-H2 unless user demand surfaces.

---

## 8. What we are NOT building

Explicit non-goals so scope stays honest:

- **Always-on screen recording** — Rewind's approach; killed by trust cliff at week 2
- **Real-time meeting transcription** — needs mic always-on; wrong wedge
- **Passive location tracking** — creepy > useful for our audience
- **Biometric / physiological data** — invasive for consent budget
- **Any cloud-synced ingestion path** — off-wedge per positioning
- **Automatic "smart" categorization surfaced as truth** — refinement UX exists precisely so we don't have to pretend NER is right
- **Sharing / social ingestion features** — off-wedge, individual-only per positioning commitment
- **Third-party ambient integrations we can't build receipts for** — if a source can't produce a per-capture receipt, we don't ingest it

---

## 9. Sequencing vs F1 + Product plans

Three plans now compete for engineering LOC. Reconciled sprint plan:

| Sprint | F1 focus | Product focus | Ingestion focus | Rough LOC split |
|-------:|----------|---------------|-----------------|-----------------|
| 1–2 | F1.1 BGE-M3, F1.2 hygiene | X1–X4 foundation | I1–I4 safety foundation | 30/30/40 (safety pays down deepest debt first) |
| 3–4 | F1.3 GEPA wire, F1.4 feedback→reward | X5–X8 screenshots + web | I5–I8 review + modes | 30/40/30 |
| 5–6 | F1.5 grammar, F1.6 EVG | X9–X12 feedback UI + voice + PDF | I9–I13 Tier N1 primitives | 30/40/30 |
| 7–8 | F1.7 multi-query, F1.8 salience | X13–X15 compose + reconcile + email | I14–I17 Tier N1 finish + N2 | 40/30/30 |
| 9–12 | F1.9 BridgeRAG, F1.10 template GEPA | X16–X20 polish + retention | I18 review + one N3 | 50/30/20 |

Guiding rule from all three plans combined: **safety and trust come first (I-P1), then a coherent product surface (X-P1), then F1 (F1 Tier 1)**. The old default of "build the substrate then wrap UI" is exactly what got us to F1 50 with an unclosable loop and no retention story. Safety + surface + F1 together, or none of them shipped.

---

## 10. Single success signal for this plan

**14-day uninstall rate ≤ 25%** among installers where at least one Focus mode was triggered.

- If uninstall stays above 25%, trust story is unbelievable regardless of surface quality — user got creeped out before they got to value.
- Focus-mode trigger is the gating condition because it proves the user actually engaged with active consent — measuring uninstall on people who never opened the app measures nothing.

If this number hits, everything else in §6 explains why. If it misses, the safety mechanisms are theater, not architecture.

---

*Companion to `PRODUCT_EXPERIENCE_PLAN.md` (surfaces), `F1_IMPROVEMENT_PLAN.md` (quality), `AIE_2026_BACKLOG.md` (deferred ideas), `CHARTER-H2-2026.md` (strategy). Not authoritative for scheduling; sprint plans in each doc must be reconciled together — see §9.*
