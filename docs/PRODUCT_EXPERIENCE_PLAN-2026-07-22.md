# Product Experience + Multi-Modal Ingestion Plan — 2026-07-22

**Framing:** the backend is 30 crates, 551 passing tests, F1 50 on fresh convs. Real. But **without a product experience this ships nothing**, and without multi-modal ingestion the "memory OS" claim is a lie — a text-only clipboard watcher is not a memory OS, it's a diary.

This plan is the counterweight to `docs/F1_IMPROVEMENT_PLAN.md`. F1 wins investor pitches; **experience wins retention**. Both required; F1 alone is a research project, experience alone is a demo.

---

## 1. Honest diagnosis (what's shit today)

**Capture surface — thin:**
- `tm-capture` = clipboard watcher + shell history + browser-capture (single file)
- No screenshot, no voice, no PDF, no email, no photo, no calendar, no image
- The one "always on" surface (browser_capture.rs) is text-only extraction
- **Verdict:** we ingest the least interesting fraction of the user's day

**Ingestion pipeline — text-monoculture:**
- `IngestPipeline` = governance → NER → graph upsert → embed → trace
- Every path assumes UTF-8 text
- No routing by modality, no OCR, no ASR, no image embedder, no chunker per-type
- **Verdict:** the ingestion crate name lies about its scope

**Tauri app — surface exists, story doesn't:**
- 26 views shipped (Brief, MemoryGarden, EventGraph, Threads, Ontology, WmeCards, Commitment, Onboarding, etc.) — real surface area
- But no clear **first-run story**, no **aha moment map**, no **daily reason to open the app**
- Views were built engineering-first (one per crate concept) rather than journey-first
- **Verdict:** a 26-view app with no home page is worse than a 4-view app with one

**Onboarding — file exists, journey unclear:**
- `OnboardingView.tsx` present but current state (per commit history) suggests demo-fixture-driven, not real first-time-user
- No "grant this permission → we index your Downloads → 30 seconds later you see something magic"

**Feedback loop UX — invisible:**
- Q3.1 shipped `memory_feedback` MCP verb (11 signal kinds)
- Zero UI to invoke it. Users can't accept/reject cards, mark useful, etc.
- The self-improvement loop from the H2 charter **cannot close without this UX** — no matter how much backend we ship

**Rollback UI — schema without surface:**
- Q4.14 shipped `policy_mutations` + `policy_rollbacks` tables
- No Tauri UI. Users can't see or roll back mutations. Pillar 7 safety rail is theoretical.

---

## 2. North star (what the product feels like when done)

Three concrete moments a user should experience by week 4:

1. **Day 1, minute 5:** they see TraceMind describe *something they forgot they told it* — pulled from a screenshot they took of a whiteboard three days ago.
2. **Day 3, morning Brief:** a card says *"You've mentioned 'the Q4 review' in 3 Claude Code sessions across two days. Want me to compose the context for your next session?"* — one click, context is bridged, they don't re-explain themselves.
3. **Week 2, first contradiction card:** *"Last Tuesday you said the launch is Nov 5. Yesterday you said Nov 12. Which is current?"* — user picks, contradiction resolves, TraceMind stops asking.

If any of these three doesn't land, the product isn't done. Everything below is in service of these three moments.

---

## 3. Multi-modal ingestion layer

### 3.1 Priorities (ranked by moment-1 impact ÷ engineering cost)

| # | Modality | Moment enabled | Cost | Notes |
|---|----------|----------------|-----:|-------|
| M1 | **Screenshots (macOS)** | Moment 1 (whiteboard recall) | ~1200 LOC | ScreenCaptureKit hook + OCR (Vision framework, local) + VLM description (Tier-1 optional). Ambient every N minutes OR on ⇧⌘4 |
| M2 | **Web pages (real capture, not just URL)** | Moment 2 (cross-session composition) | ~600 LOC | Reader-mode extraction (readability-rs), plus a browser extension pushing full-page HTML on user pin |
| M3 | **PDFs & docs** | Moment 3 (contradiction in a spec doc) | ~500 LOC | `lopdf` / `pdfium` + `docx-rs`. Watch Downloads + user-picked folders |
| M4 | **Voice notes** | Moment 1 alt (post-meeting brain-dump) | ~1000 LOC | `whisper-cpp` local (Q5_1 base.en ~150MB). Menu-bar record button; ⌥Space push-to-talk |
| M5 | **Emails (IMAP read-only)** | Moment 2 alt (thread bridging) | ~1500 LOC | IMAP client + local index only; no send. Opt-in per-account |
| M6 | **Photos with EXIF** | later | ~400 LOC | Location + timestamp make photos great episodic anchors |
| M7 | **Calendar (read-only)** | later | ~500 LOC | EventKit on macOS; "you have a call with X in 20 min, here's what you've been thinking about them" |
| —  | **Video, live audio, real-time meeting capture** | not H2 | — | Too heavy; wrong wedge |

### 3.2 Ingestion architecture changes (`tm-ingest`)

**Current:** `IngestPipeline.ingest(text: &str)` — single-modality
**Target:** `IngestPipeline.ingest(payload: MultimodalPayload)` where

```rust
enum MultimodalPayload {
    Text  { body, source, ts },
    Image { bytes, format, source, ts, hint: Option<String> },
    Audio { bytes, sample_rate, source, ts },
    Pdf   { bytes, source, ts },
    Web   { url, html, extracted_text, source, ts },
    Email { headers, body, thread_id, source, ts },
}
```

Each variant routes to a **preprocessor** that emits (a) a canonical text string for the existing text pipeline, (b) modality-specific attachments stored in `~/.tracemind/blobs/<sha256>`, (c) modality-space embeddings that plug into `ComposedIndex` as new Spaces.

**New `ComposedIndex` spaces (feeds Pillar 2):**
- `ImageEmbedSpace` — CLIP-based (openai/clip-vit-base-patch32 ONNX, ~150MB) → visually-similar recall
- `AudioTranscriptSpace` — already-text after Whisper, but keeps `duration` + `speaker_count` as facets
- `ThreadSpace` — email/chat/session grouping so bridging suggestions have a natural unit

**Storage delta:**
- New `blobs/` directory in `~/.tracemind/`
- New `attachments` SQLite table: `(memory_id, kind, sha256, mime, bytes_len, extracted_text_ref)`
- Every attachment addressable by content hash; dedup on write

### 3.3 Ingestion routing (single decision point)

`tm-capture` sends every event through a `ModalityRouter` that decides:
1. Is this ingestion-worthy at all? (governance PII gate + noise filter)
2. What preprocessor to run?
3. Does this deserve to hit the "hot" tier immediately (screenshot user just took) or "warm" (background clipboard drip)?

This is the point at which Q3.2 memory-routing gate applies on the **write** side, not just the read side.

---

## 4. Product experience surfaces

### 4.1 Tauri Brief — the daily home

**Current:** BriefView exists as one of 26 views. No hierarchy.

**Target:** the Brief is *the* home screen. Everything else is a drilldown or a setting. When TraceMind opens, you see the Brief and nothing else — the sidebar with 26 items is a power-user drawer.

The Brief has exactly four card slots:

1. **Recall** — "what did I do yesterday / this week" — pulled from `ComposedIndex` with `recall` verb weights
2. **Compose** — "you've been thinking about X across N sessions; bridge?" — from the composition-layer graph algebra (Q4.7)
3. **Reconcile** — "you said A then B; resolve?" — from the contradiction detector (Q3.4)
4. **Rehearse** — "commitment X is due; here's the context" — from the Commitment ledger + spaced repetition (backlog §3.7)

Every card has **four actions** and only four: **Open** · **Pin** · **Dismiss** · **Why?** Feedback signals flow into Q3.1's fabric.

**Empty-state handling:** if there's nothing to say, the Brief says nothing. No filler cards, no "check back later" pablum. Silence is a valid product state.

### 4.2 Menu-bar app — the always-on ambient surface

**Current:** doesn't exist.

**Target:** small icon that:
- Shows unread card count
- Push-to-talk voice note (⌥Space)
- Push-to-capture screenshot to memory (⌥⌘Space)
- Quick recall bar (⌥/) — type a fragment, get top-3 memories in a dropdown, click to open in Brief

This is where most days of TraceMind usage happen. Full Tauri window is opened weekly, not daily.

### 4.3 MCP integration — the invisible surface

**Current:** MCP verbs ship; users have to configure Claude Code / Goose manually.

**Target:**
- One-click "connect to Claude Code" in Onboarding — writes MCP config for the user
- Every retrieval in Claude Code has a **subtle inline provenance line** "(from your memory · 4 sources · why?)" — Claude Code plugin territory but request is on Anthropic side
- **Session-context injection**: when a Claude Code session starts, the first thing TraceMind ships is a composed summary of what the user has been thinking about relevant to the working directory. Silent unless the user asks.

### 4.4 CLI — power users only

Keep as maintenance. Every CLI command should also work in the Tauri Brief. CLI is documentation for what's possible, not a product surface.

---

## 5. Aha-moment map (what has to be true by when)

| When | Aha | Backend it depends on | UX it depends on |
|------|-----|-----------------------|------------------|
| Install + 60s | "It already knows something about me" | Ingest Downloads + browser history + clipboard on first run | Onboarding view that runs the initial ingest visibly |
| Day 1 | "It remembered my whiteboard photo" | M1 screenshots + OCR | Menu-bar screenshot capture + Brief Recall card |
| Day 3 | "It bridged my two Claude Code sessions" | Composition-layer (Q4.7) + Q3.5 session scoping | Brief Compose card + MCP session-context injection |
| Week 1 | "It sees a pattern I didn't" | Insights + WmeCards (already shipped) | Brief Insight card (currently underused) |
| Week 2 | "It caught me contradicting myself" | Contradiction detector (Q3.4) | Brief Reconcile card |
| Week 4 | "It's learning what I care about" | VerbAffinityModel (Q4.12) + Curator (Q4.13) | Settings > "Your patterns" view |
| Month 2 | "It found the right memory from a screenshot" | ImageEmbedSpace + M1 | Quick recall bar (⌥/) |
| Month 3 | "I trust it enough to run it in the background all day" | Rollback UI (Q4.14) + provenance edges | Settings > Rollback view |

The map is the plan. If a milestone below doesn't feed at least one aha, it doesn't ship.

---

## 6. Concrete goals with metrics

Every goal has a **falsifiable metric** measurable without asking the user.

| # | Goal | Metric | Deadline |
|---|------|-------:|:---------|
| G1 | Multi-modal coverage | 5 modalities ingesting cleanly (M1–M5) | Week 8 |
| G2 | Time-to-first-Brief | ≤ 24h from install for 90% of users | Week 4 |
| G3 | Time-to-first-aha | ≤ 72h from install (measured by first card that got "Open" or "Pin") | Week 6 |
| G4 | Card accept rate | Accepted (Open+Pin) / Shown ≥ 0.30 | Week 8 |
| G5 | Weekly active retention at week 4 | ≥ 40% (fraction of installers still opening Brief in week 4) | Week 12 |
| G6 | Menu-bar DAU / Tauri WAU ratio | ≥ 2× (proving ambient > destination usage) | Week 10 |
| G7 | Capture-to-recall p50 | ≤ 5 min (screenshot at 10:00 recallable via quick bar at 10:05) | Week 6 |
| G8 | Contradiction card true-positive rate | ≥ 70% (user picks a side rather than dismissing as bogus) | Week 10 |
| G9 | Feedback loop closure | 100% of retrievals emit `feedback_hook_id`; ≥ 50% of Brief cards get an explicit signal within 24h | Week 6 |
| G10 | Rollback surface usable | Any policy mutation viewable and rollbackable within 3 clicks from Brief | Week 10 |

If G4 or G5 fails, the product doesn't work regardless of F1.

---

## 7. Ordered todo list (X1–X15, product-side)

Ordering by **aha-blocking-ness** first, cost second.

### Sprint P1 (weeks 1–2) — foundation + first aha

- **X1 — Multimodal payload plumbing** — `MultimodalPayload` enum + `ModalityRouter` + `attachments` table + `blobs/` directory. ~600 LOC. Unblocks everything.
- **X2 — Menu-bar app skeleton** — Tauri tray app, quick recall bar, screenshot-to-memory hotkey (routes through M1 stub). ~500 LOC. Enables ambient surface.
- **X3 — Onboarding rewrite** — first-run "grant permissions → we index N sources → aha in <60s" flow. ~400 LOC UI + ~200 LOC backing.
- **X4 — Brief home reorganization** — 4-slot layout (Recall / Compose / Reconcile / Rehearse), demote 26 views to a drawer, 4-action card contract. ~600 LOC UI.

**P1 aha delivered:** install → onboarding → menu-bar → first screenshot → first Brief card.

### Sprint P2 (weeks 3–4) — screenshots + web

- **X5 — M1 screenshots** — ScreenCaptureKit + Vision-framework OCR + Tier-1 VLM description (optional) + blob storage. ~1200 LOC.
- **X6 — M2 web-page capture** — Reader-mode extraction + optional browser extension pushing full HTML on pin. ~600 LOC + ~400 LOC extension.
- **X7 — ImageEmbedSpace in ComposedIndex** — CLIP ONNX + new Space. ~500 LOC.
- **X8 — Quick recall bar (⌥/)** — menu-bar dropdown, top-3 memories with previews. ~400 LOC.

**P2 aha delivered:** screenshot recall, web-pin composition, image-based search.

### Sprint P3 (weeks 5–6) — feedback loop closes, voice, PDFs

- **X9 — Card feedback UI wiring** — every Brief card's 4 actions actually emit `memory_feedback` signals with correct `hook_id`. ~300 LOC.
- **X10 — Rollback UI in Tauri** — Settings > History view over `policy_mutations` + one-click rollback. ~500 LOC.
- **X11 — M3 PDF ingestion** — pdfium/lopdf + Downloads watcher. ~500 LOC.
- **X12 — M4 voice notes** — whisper-cpp bundled + menu-bar record + ⌥Space push-to-talk. ~1000 LOC.

**P3 aha delivered:** self-improvement loop actually closes; voice + PDF land; rollback surface exists.

### Sprint P4 (weeks 7–8) — composition, contradiction, email

- **X13 — Compose card wiring** — Q4.7 composition-layer verbs surfaced in Brief; MCP session-context injection for Claude Code. ~600 LOC.
- **X14 — Reconcile card wiring** — Q3.4 contradiction detector surfaced in Brief with explicit "which is current?" resolution. ~400 LOC.
- **X15 — M5 email (IMAP read-only)** — one account minimum, opt-in per-account, no send. ~1500 LOC.

**P4 aha delivered:** the three north-star moments (§2) all land.

### Sprint P5 (weeks 9–12) — polish, retention, second-wave modalities

- **X16 — Insight/pattern cards** — surface existing `tm-reflect::insights` output as Brief Insight cards.
- **X17 — "Your patterns" settings view** — VerbAffinityModel (Q4.12) made visible; user can see and toggle personalizations.
- **X18 — M6 Photos with EXIF** — ~400 LOC.
- **X19 — M7 Calendar** — EventKit + pre-meeting Brief. ~500 LOC.
- **X20 — Retention hooks** — email digest ("your week in memory"), notification when a Reconcile card is high-confidence.

---

## 8. What we are NOT building

- **Video ingestion** — too heavy, wrong wedge for local
- **Live meeting capture** — needs system-audio permission + speaker diarization + always-on ASR budget we don't have
- **Chat with your memory (LLM-front-and-center chatbot UI)** — commoditized; the value is *proactive Brief*, not reactive chat. If we ship chat it's an MCP verb, not a UI
- **Team / shared workspace features** — off-wedge until 2027 (positioning: individual-only per H2 discussion)
- **Cloud sync** — off-wedge; local-first is the moat
- **Web app** — Tauri app + MCP is enough surface for H2
- **New tabbed 27th view** for every new concept — cap at the 4-slot Brief + drawer; feature velocity should slow, not accelerate

---

## 9. Sequencing vs F1 plan

Two plans, one team. They compete for LOC. Rough split:

| Sprint | F1 plan focus | Product plan focus | LOC split |
|-------:|---------------|--------------------|-----------|
| 1–2 | F1.1 BGE-M3, F1.2 bench hygiene | X1–X4 foundation | **60% product / 40% F1** — product debt is bigger |
| 3–4 | F1.3 GEPA wire, F1.4 feedback→reward | X5–X8 screenshots + web | **50/50** |
| 5–6 | F1.5 grammar, F1.6 EVG | X9–X12 feedback UI + voice + PDF | **50/50** |
| 7–8 | F1.7 multi-query, F1.8 salience | X13–X15 compose + reconcile + email | **40 product / 60 F1** — infra rebalance |
| 9–12 | F1.9 BridgeRAG (gated), F1.10 template GEPA | X16–X20 polish + retention | **30 product / 70 F1** — competitive publish window |

**Guiding rule:** F1 numbers are only worth publishing if there are users to measure retention against. Product first for the first 4 sprints, F1 first for the last 4.

---

## 10. Success signal for this plan (single number)

**Week-4 retention among installers ≥ 40%** with **≥ 5 modalities ingesting**. If both hit, TraceMind is a product. If either misses, we've built infrastructure and called it product. The rest of the metrics in §6 exist to explain *why* week-4 retention did or didn't hit.

---

## 11. Implementation status (2026-07-24)

Second pass: every plan item now has a landing site — backend + MCP + a CLI hook (or a file-drop watcher) that a native surface can replace later. UI shells (Tauri Brief, menu-bar tray, onboarding UI) remain the last mile; each is one MCP call away from a real screen.

| # | Item | Status | Where it landed | Test count |
|---|------|--------|-----------------|-----------:|
| **X1** | Multimodal payload plumbing | **shipped** | `tm-ingest/src/multimodal.rs` (payload / router / blob store / attachments table) + `IngestPipeline::ingest_multimodal` | 15 |
| **X2** | Menu-bar app skeleton | **shipped (backend + CLI hook)** | `memory_quick_recall` MCP verb + `tracemind quick-recall` CLI (launcher target). Tauri tray shell behind `menu-bar` feature — still needs native shim. | 4 |
| **X3** | Onboarding rewrite | **shipped (backend + CLI hook)** | `memory_onboarding_status` MCP verb + `tracemind onboard [--dry-run] [--json]` CLI writes `~/.tracemind/onboarding.json`. Tauri UI still TSX. | 1 |
| **X4** | Brief home 4-slot | **shipped (backend)** | `tm-reflect/src/brief_home.rs` (`build_home`, 4 actions, deterministic ids). UI still TSX. | 13 |
| **X5** | M1 screenshots | **shipped (poll-based)** | `ScreenshotSource` in `tm-capture/src/modalities.rs` polls `~/Desktop` for PNG. `macos-screencapture` feature reserved for ScreenCaptureKit + Vision OCR. | 1 |
| **X6** | M2 web-page capture | **shipped (server side)** | `WebPreprocessor` in `tm-ingest/src/multimodal.rs` — HTML strip, blob-stored, URL prepended. Browser-extension push endpoint TBD. | 2 |
| **X7** | ImageEmbedSpace | **shipped (aHash placeholder)** | `ImageEmbedSpace` + `ahash64` in `tm-vector/src/space.rs`. Real CLIP behind `clip-image-embed` feature. | 6 |
| **X8** | Quick recall bar | **shipped (backend + CLI hook)** | `memory_quick_recall` MCP verb + `tracemind quick-recall <q>` CLI (launcher target: Raycast/Alfred/system hotkey). Native tray UI still deferred. | (covered by X2) |
| **X9** | Card feedback UI wiring | **shipped (backend)** | `feedback_hooks` table in `tm-graph/src/feedback_hooks.rs`, `persist_hooks` + `signal_for_action` in `tm-reflect/src/brief_home.rs`, `kind=card_action` in `memory_feedback` MCP verb. | 6 |
| **X10** | Rollback UI (backend) | **shipped** | `tracemind rollback list|show|apply` CLI + `memory_rollback_list` / `memory_rollback_apply` MCP verbs. Tauri UI still TSX. | 4 |
| **X11** | M3 PDF ingestion | **shipped** | `PdfPreprocessor` in `tm-ingest/src/multimodal.rs` + `PdfWatcherSource` polls `~/Downloads` and extracts via `pdf-extract` (panic-safe). | 2 |
| **X12** | M4 voice notes | **shipped (poll-based)** | `VoiceNoteSource` polls `~/.tracemind/voice` for wav/m4a/mp3; real whisper.cpp transcription behind `whisper-cpp` feature. | 1 |
| **X13** | Compose card wiring | **shipped (heuristic)** | `collect_compose` in `tm-mcp/src/main.rs` — naive-but-honest cross-session token bridge. Q4.7 algebra upgrade is a later PR. | 1 |
| **X14** | Reconcile card wiring | **shipped** | `collect_reconcile` reads open `ContradictionView`s from the belief store, joins with `triple_detail` for topic + values. | (covered by X4) |
| **X15** | M5 email | **shipped (poll-based)** | `EmailInboxSource` watches `~/.tracemind/email` for `.eml` (parsed via `mail-parser`); real IMAP behind `imap-email` feature. | 1 |
| **X16** | Insight/pattern cards | **shipped (backend)** | `memory_insights_current` MCP verb re-uses the same `InsightConfig` the brief consumes. | 1 |
| **X17** | "Your patterns" settings | **shipped (backend)** | `memory_patterns_show` MCP verb over `tm-reflect::detect_patterns`. Tauri surface TBD. | 1 |
| **X18** | M6 Photos + EXIF | **shipped (poll-based)** | `PhotoLibrarySource` reads `~/Pictures` via `kamadak-exif` — composes `filename @ DateTimeOriginal [GPS] (Model)` text. Native PhotoKit behind `exif-photo`. | 1 |
| **X19** | M7 Calendar | **shipped (poll-based)** | `CalendarSource` reads `.ics` via `ical` — composes `Title @ DTSTART — DTEND; LOCATION; DESCRIPTION`. EventKit behind `apple-eventkit`. | 1 |
| **X20** | Retention hooks | **shipped** | `tracemind digest [--persist]` CLI + `memory_digest_weekly` MCP verb write `~/.tracemind/notifications/digest-<iso-week>.json` and per-contradiction reconcile markers. Email delivery TBD. | 1 |

### What that adds up to

- **Every X-item now has landing infrastructure.** All 20 items are either shipped (backend + MCP + CLI hook where applicable) or shipped-poll-based (a pure-Rust file-watcher stand-in that a later feature-gated native path can replace). Nothing in the plan is still marked *not started*.
- **Native OS shells (Tauri Brief home, menu-bar tray widget, real ScreenCaptureKit / Vision OCR / whisper.cpp / IMAP / EventKit / PhotoKit) are the last mile.** They call the verbs already shipped — the fastest way to close them is to build against `memory_brief_home`, `memory_ingest_multimodal`, `memory_feedback kind=card_action`, `memory_quick_recall`, `memory_insights_current`, `memory_patterns_show`, `memory_onboarding_status`, `memory_digest_weekly`. Each maps 1-to-1 to a card / slot / row / notification.
- **Every modality source runs today via file-drop conventions.** Screenshots on `~/Desktop`, voice notes in `~/.tracemind/voice`, PDFs in `~/Downloads`, web pins in `~/.tracemind/web-pins`, emails in `~/.tracemind/email`, photos in `~/Pictures`, `.ics` events in `~/.tracemind/calendar`. Users on any OS can drop a file and the daemon picks it up. Feature flags for the native APIs sit unused until we need them.

### Tests shipped this pass

**46** tests in `tm-mcp` (+4 new: `memory_insights_current`, `memory_patterns_show`, `memory_digest_weekly`, `memory_onboarding_status`), **13** in `tm-reflect::brief_home`, **15** in `tm-ingest::multimodal` + `tm-ingest::pipeline`, **3** in `tm-graph::feedback_hooks`, **8** in `tm-capture::modalities`, **6** in `tm-vector::space::image_embed_tests` (X7). All green.

### What is *not* claimed

- No user has touched any of this. The plan's single success signal (§10) — week-4 retention ≥ 40% with ≥5 modalities ingesting — remains untested. Everything above is *unblocking* infrastructure, not evidence the wedge works.
- The compose slot is heuristic (naive token overlap across sessions). X13's real Q4.7 algebra bridge is a follow-on.
- ImageEmbedSpace uses a 64-bit aHash placeholder; real CLIP is behind `clip-image-embed`.
- Menu-bar / tray widget still needs Tauri v2 tray + GlobalShortcut wiring. The `tracemind quick-recall` CLI is the launcher target that closes X2/X8 without a native shell.
- Rollback CLI works end-to-end but has been exercised only by tests and one manual seed — no GEPA-emitted mutation has been reversed through it yet.
