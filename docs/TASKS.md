# TraceMind — Task List

**Reframed 2026-05-11 against the seed bar.** Every priority below is judged by one question: *does this get me closer to a user on camera saying "I won't go back"?* That sentence is the only thing that unlocks a seed check. Engine depth, three-product fan-out, and architectural elegance are graded *only* by whether they shorten the distance to that moment.

> **Canonical strategy doc:** [`docs/PROJECT_2026.md`](PROJECT_2026.md). This task list operationalises that plan. When they conflict, fix both.

Legend: `[x]` done, `[-]` in progress / partial, `[ ]` not started.

---

## North star — the seed gate

A Sequoia seed check requires three artifacts. Until all three exist, nothing else matters:

1. **One named design partner** using TraceMind daily, on camera, naming the pain (DP-6).
2. **One retention number** — W2 ≥ 40% across at least 5 users (DP-3 + DP-5).
3. **One head-to-head video** — TraceMind vs. Mem0 / Letta / Zep, same `claude` host, three beats: (a) persistence across sessions, (b) context isolation, (c) contradiction-aware retraction (W-5). Persistence + context-isolation lead; retraction is the closing moat beat.

All P0–P3 below feed exactly these three artifacts. Everything from P4 down is deferred until the gate is cleared.

---

## Shipped (reference, do not re-litigate)

**Engine + storage**
- [x] 24-crate Rust workspace, full test suite green
- [x] Bitemporal substrate (`tm-temporal` embedded into `tm-graph`)
- [x] TMS-backed confidence + contradiction surfacing (Sprint C-2)
- [x] Commitment primitive + state machine + outcome attachment
- [x] Daily brief + insights + pattern detector
- [x] World model v0 (`f_outcome` logistic regression, preflight)
- [x] Tier-0 extractive answerer + Tier-1 / Tier-2 scaffolds
- [x] ColBERT rerank, BGE-small embeddings, 5-arm LinUCB bandit
- [x] Reasoning: chains, analogy (WL kernel), causal trace, consolidator
- [x] Capture daemon, Tauri desktop shell, LoCoMo bench harness

**Context + feedback (Sprints C / D / F-1)**
- [x] Sprint C-0: context segmentation (schema, CRUD, ingest tagging, scoped retrieval, cross-context penalty, negative-feedback CLI, demo rewrite)
- [x] Sprint D investor-UI: query_id surface, context switcher, inline 👍/👎/wrong-ctx, demo fixture restore, smoke script
- [x] F-1 positive-signal CLI + storage + MCP (helpful → bandit reward)
- [x] D-1..D-6 recordable demo path (script, fixture, pre-roll, brief in Tauri, single-binary install, product-close doc)
- [x] UX polish 2026-05-11: sticky recommendations, reason details, origin context, cold-start reasoning engine

**MCP host integration (P0a, 2026-05-11)**
- [x] MCP-1 Claude Code integration doc — `docs/CLAUDE_CODE_INTEGRATION.md`
- [x] MCP-2 Goose integration doc — `docs/GOOSE_INTEGRATION.md`
- [x] MCP-3 One-command Claude Code installer — `scripts/install_claude_code.sh` (`--project`, `--no-hint`, `--no-demo` flags)

**Tauri investor UI (P0c, Sprint D 2026-05-10)**
- [x] UI-1..UI-6 query_id surface, active-context switcher, inline 👍/👎/wrong-ctx buttons, demo fixture restore, smoke script

---

## P0 — Two surfaces, one engine (MCP wedge → Tauri flagship)

**Strategic frame (2026-05-11, after investor input + second-pass review):** TraceMind ships *two complementary surfaces*, both P0:

- **MCP wedge (P0a)** — `tm-mcp` plugs into Claude Code, Goose, Cline, Cursor. This is the developer wedge — Sequoia partners install in 30 seconds during the meeting; design-partner recruiting runs through MCP-host communities.
- **Tauri flagship (P0c)** — the consumer surface where the user goes when they want to *see* their memory: brief panel, context switcher, contradiction badges, calibration view. The MCP integration is the *first* surface; the Tauri app is where users live longer-term.

Two surfaces tell a stronger story than one — Vercel led CLI → dashboard, Linear led Mac app → web, Cursor led editor → enterprise. The MCP-only framing risks repositioning the company as a memory plugin (a Plaid) instead of a memory product (a Notion). Both surfaces stay P0 until W2 retention clears the seed gate, at which point Tauri investment intensifies.

### P0a — MCP host distribution (the channel)

MCP-1..MCP-3 shipped 2026-05-11 (see Shipped above). Open work:

- [ ] **MCP-4 One-command Goose installer** — `scripts/install_goose.sh`. Mirror of MCP-3 for Goose: edits `~/.config/goose/config.yaml`, writes `.goosehints`, restores fixture, prints the 4-prompt demo.
- [ ] **MCP-5 Submit to Anthropic's MCP servers directory + Goose extensions registry** — both maintain public lists of MCP servers. Landing on those lists is free organic distribution.
- [ ] **MCP-6 One-sentence pitch propagated** — replace any "system of intents" / "contradiction-aware-first" / "never blurred" lead copy on `README.md`, `tracemind.dev`, MCP host directory entries with the W-7 wedge sentence: *"Ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded."* Cross-ref P1 W-7 (single source of truth for the line).
- [ ] **MCP-7 Cline + Cursor integration docs** — 30-minute write-ups each. Cline is MCP-native and ships in VS Code; Cursor's MCP support is recent but landed. Both have aggressive early-adopter communities — direct DP-1 recruiting channels. Scheduled Q3 per PROJECT_2026.md.

### P0b — Design partner recruitment (the goal)

Without a named outside user using TraceMind every day, no later work compensates. This is the rate-limiting step. The integration docs above (MCP-1..MCP-3) **are** the onboarding kit — DP-2 below is no longer a blocking write-up, it's a polish + screen-recording job.

- [ ] **DP-1 Recruit list** — name 10 candidates. Now drawn from MCP host communities: Claude Code power users on r/ClaudeCode + Anthropic Discord, Goose Discord regulars, Cline GitHub stargazers, founders under NDA, therapists, coaches, researchers, journalists, IP lawyers. Personal email/DM each one, *not* a broadcast post.
- [-] **DP-2 Onboarding kit** — `docs/CLAUDE_CODE_INTEGRATION.md` + `docs/GOOSE_INTEGRATION.md` + `scripts/install_claude_code.sh` cover the written kit. Remaining: 60-second screen recording showing `bash scripts/install_claude_code.sh` → `claude` session → retraction beat firing on prompt 3. Single `curl | sh` line at the top of the kit.
- [ ] **DP-3 Instrumentation (privacy-preserving, local-only)** — local-only daily-active flag in `~/.tracemind/usage.json`: timestamp of last query, last `helpful` signal, last `not-related` signal. User opts in to share via `tracemind share-usage --to <email>` which prints the JSON for them to paste back. No telemetry.
- [ ] **DP-4 Weekly check-in script** — 15-minute call per design partner, weekly: "what did you ask Claude/Goose this week? what did TraceMind get wrong? what would make you uninstall the MCP?" Write it down. This is the dataset.
- [ ] **DP-5 W2 retention gate** — 5 partners onboarded by 2026-06-15. W2 retention measured by `usage.json` returns ≥ 3 active days in week 2 from at least 3 of 5. Below that → the wedge is wrong, stop building, re-brainstorm.
- [ ] **DP-6 Testimonial video** — 60-second on-camera from the partner with the strongest W2: pain → what TraceMind does inside their Claude Code / Goose session → "I won't go back." This is slide 1 of the seed deck.

**Exit criteria:** DP-6 captured. Without it, do not advance to P3 or beyond.

### P0c — Tauri consumer surface (the flagship)

The Tauri app is where the user goes when they want to *see* memory, not just *use* it inside a chat. It must be visibly product-grade by the seed pitch — slide 2 of the deck is a Tauri walkthrough.

UI-1..UI-6 + 2026-05-11 UX fixes shipped (see Shipped above). Open work:

- [ ] **UI-7 60–90s Tauri walkthrough screen recording** — human capture session. Slide 2 of seed deck. Sprint D ends when this exists.
- [ ] **UI-8 First-run onboarding flow** — sample data → meaningful brief in 60 seconds. Critical for any non-developer DP candidate.
- [ ] **UI-9 Brief panel polish** — read-on-open, dismissable, archived. C-0.9 (active context in brief) lives here.
- [ ] **UI-10 Commitment timeline view** — vertical, color-coded by state, drawer on click. The intent-arc story needs a surface. PROJECT_2026.md Q4 commitment.
- [ ] **UI-11 Calibration panel** — predictions made, outcome accuracy, Brier score. The "world model that works" slide. PROJECT_2026.md Q4 commitment.
- [ ] **UI-12 Settings + privacy panel** — clear data, export, opt-in toggles for any future telemetry. Critical for trust-signaling to privacy-conscious DPs.
- [ ] **UI-13 Capture-permissions panel (Q2, seed-critical)** — per-source toggle for clipboard / shell / screenshot / browser / audio / calendar; "last captured at" timestamp; event-count chip; one-click "forget all captures from this source." Powers CAP-1.
- [ ] **UI-14 Context-switch suggestion banner** — when CTX-2 fires, Tauri brief shows a dismissable banner: "This might belong in your *work* context. Switch?" One-click switch + ingest in new context. Negative dismiss triggers `wrong_context_suggestion` feedback.

**Exit criteria:** UI-7 recorded, UI-8 shipped, the Tauri app is the answer to *"after they install the MCP, where do they spend time?"*

---

## P1 — The wedge proof (persistence + ambient capture + adaptive context, retraction as moat)

The user-felt wedge — what makes someone say "I won't go back" in week 1 — is *persistence + ambient capture + adaptive context*. Contradiction is the *architectural moat* that protects the wedge from being commoditised. The head-to-head must lead with the wedge demos and use retraction as the closing differentiator.

The format is **same host (Claude Code), different MCP memory servers** — not different apps. Four `claude` sessions side-by-side, each with a different memory MCP plugged in, same prompts in, different outcomes out — plus a fifth pane showing the Tauri brief with ambient-captured entities from clipboard/shell/screenshot.

### Wedge proof (lead with this)

- [ ] **W-1 Persistence head-to-head** — same 90-second Claude Code scenario (fact stored in session A, recalled in fresh session B with a different working directory) run against Mem0, Letta, Zep, TraceMind. Document each failure mode on the fresh-session recall prompt. Save transcripts.
- [ ] **W-2 Context-aware head-to-head** — three sub-scenarios on a 100-pair labeled set: (a) **correct isolation** — fact stored under context A, query in context B → nothing returned; (b) **correct bridge** — same person/concept genuinely present in both contexts → bridge fires with a visible label; (c) **correct switch suggestion** — user queries with content that scores poorly in active context but high in another → TraceMind proposes switching contexts. Score: TP-bridges − FP-bridges; target ≥ 92%. No competitor proposes context switches; document.
- [ ] **W-3 Cross-session persistence microbenchmark** — 50 hand-labeled (session-A-store, session-B-query) pairs across 5 categories. Target ≥ 90% TraceMind.
- [ ] **W-8 Ambient capture wedge demo** — record a 60-second flow: install TraceMind, work normally for 10 minutes (copy a few snippets, run shell commands, take 2 screenshots), then ask Claude Code about something you did. TraceMind answers from auto-captured entities; competitors return nothing because they were never told. This is the *"I never typed any of this in"* beat.

### Moat proof (close with this)

- [ ] **W-4 Retraction microbenchmark** — 50 hand-labeled commitment/retraction pairs with adversarial paraphrases (negation, time-shift, partial retraction). Target ≥ 80% TraceMind; competitors near 0%.
- [ ] **W-5 Side-by-side recording** — 4-pane Claude Code capture + 1-pane Tauri brief, 90–120 seconds, four beats: (1) persistence recall in a fresh session, (2) adaptive context (isolation + switch suggestion), (3) ambient-captured entity surfacing, (4) retraction. No narration. Posted publicly on launch.

### Public artefacts

- [ ] **W-6 Public leaderboard page** — `tracemind.dev/memory-bench` shows W-3 (persistence) + W-2 (context-aware) + W-4 (retraction) + W-8 (capture coverage demo) scores for TraceMind and each competitor. Monthly refresh. Code in `crates/tm-bench-memory/`.
- [ ] **W-7 One-sentence wedge propagated** — replace any "system of intents" / "contradiction-aware-first" / "never blurred" lead copy with: *"Ambient memory for every AI you use — captures what you do, scopes itself to the right context, learns your boundaries, never uploaded."* README, deck slide 1, `tracemind.dev`, MCP tool descriptions, integration docs.

**Exit criteria:** W-5 video live, W-3 ≥ 90%, W-2 ≥ 92%, W-8 demo recorded, W-7 propagated. Retraction (W-4) is the second-slide moat number, not the headline.

---

## P1b — Ambient capture surface (the wedge moment)

Promoted from P10 (deferred) on user feedback (2026-05-11): *people will not hand-feed memory; we must auto-populate as much as possible with explicit per-source permissions.* This is the actual *"memory just is"* product, not a future indulgence. Without it, the wedge collapses to "a place to type things you'd otherwise type into ChatGPT memory" — not differentiated.

### Q2 — seed-critical (text-only ambient sources)

- [x] **CAP-1 Per-source permissions schema** — `~/.tracemind/capture_permissions.toml`, keyed by source (clipboard, shell, notes, screenshot, browser, audio, calendar). Each entry: `enabled`, `granted_at`, `last_event_at`, `event_count`. Tauri settings panel (UI-13) reads/writes this. CLI mirror: `tracemind capture {enable,disable,status} <source>`. *(shipped 2026-05-11: schema in `tm-types::capture_permissions`, 9 unit tests, CLI subcommand wired, capture daemon fails closed on disabled sources; notes source added 2026-05-11.)*
- [x] **CAP-2 First-run backfill (shell + notes + clipboard)** — `tracemind capture backfill --days N --max M` ingests three text sources in one shot:
  1. **Shell** — scans `~/.zsh_history` / `~/.bash_history`, filters by timestamp (zsh) + triviality (ls/cd/etc.).
  2. **Notes** — macOS only. Dumps Apple Notes via `osascript` (folder + title + body + modification date), filters by `--days`.
  3. **Clipboard** — one-shot `pbpaste` snapshot of current clipboard contents so the first query post-install isn't empty.
  Per-step opt-outs `--no-shell` / `--no-notes` / `--no-clipboard`. Each step fails closed against the corresponding `CaptureSource` permission and bypasses CAP-5 via `RateLimiter::unlimited()`. *(shell shipped 2026-05-11; notes + clipboard added 2026-05-11.)*
- [x] **CAP-4 Browser bookmarklet / extension stub** — minimal: a bookmarklet posts current `{url, title, selection}` to `http://127.0.0.1:7710/capture` (loopback only, Bearer-token gated). Full WebExtension lands in Q3. *(shipped 2026-05-11: hand-rolled HTTP/1.1 server in `tm-capture::browser_capture`, fail-closed on `CaptureSource::Browser`, per-install token at `~/.tracemind/capture_token` chmod 0600, 1 MiB body cap, `GET /health` unauthenticated for probe; CLI `tracemind capture bookmarklet` emits the install snippet. Curl smoke test: `/health` 200 → `/capture` 200 stored.)*
- [x] **CAP-5 Capture-aware ingestion throttle** — `IngestPipeline.ingest_fast` now consults a per-source `RateLimiter` (token bucket, defaults: clipboard 60/min burst 30, shell 120/burst 60, notes 30/burst 15, screenshot 12/burst 6, browser 60/burst 30, audio 30/burst 15, calendar 6/burst 3, unknown 30/burst 15). Env override: `TM_RATE_<SOURCE>_PER_MIN` / `TM_RATE_<SOURCE>_BURST` (set both to 0 to disable). Rate-limited calls return `skipped: Some("rate-limited: …")`. Bulk paths (`import`, `capture backfill`) bypass via `RateLimiter::unlimited()`. Content-hash dedup remains permanent at the pipeline level. *(shipped 2026-05-11: 5 unit tests pass, full ingest suite 50/50.)*

### Q3 — coverage expansion

- [ ] **CAP-6 Audio capture (Whisper-tiny, opt-in hotkey)** — global hotkey starts/stops; Whisper-tiny runs on-device; transcript ingested as `Capture::Audio`. No always-on listening unless explicitly enabled. Hotkey configurable.
- [ ] **CAP-7 Calendar import** — macOS EventKit / Google Calendar OAuth (read-only). Events become entities; attendees become relationships; agenda joins the brief.
- [ ] **CAP-8 Full browser extension** — replaces CAP-4 bookmarklet. Captures: visited pages, dwell time, copied text, "save to TraceMind" button. Permissions UI built-in.
- [ ] **CAP-9 IDE telemetry capture** — VS Code / Cursor extension surfaces file-open / file-edit events as low-priority entities. Cross-references with shell git events for cohesive code-context recall.

### Q4 — multi-modal + cross-modal join

Multi-modal capture (vision, audio, OCR) is deferred to Q4 — the on-device model footprint (Tesseract + Moondream / SigLIP) and the false-positive risk on screenshots that contain credentials make it the wrong wedge for seed. CAP-1/2/4/5 + W-8 demo carry us to seed on text alone.

- [ ] **CAP-3 Screenshot OCR + caption capture** *(was Q2, deferred to Q4 on 2026-05-11)* — opt-in. On screenshot capture (system shortcut), pipe through Tesseract OCR + Moondream / SigLIP for caption, ingest as `Capture::Screenshot { ocr_text, caption, sha256 }`. Stored in `~/.tracemind/captures/screenshots/` referenced by hash; raw images never leave device. Privacy: skip captures that contain detected password fields / credit-card OCR. Lands alongside CAP-10 because cross-modal entity edges are the value unlock for visual capture.
- [ ] **CAP-10 Cross-modal entity edges** — captures from screenshots / audio / code share entities (mention "Pat from Sequoia" in audio → links to a screenshot caption with that name → links to a calendar invite). Cross-modal pipeline (PROJECT_2026 Q4) consumes from these capture pipelines. Blocked on CAP-3 (screenshot OCR) + CAP-6 (audio).

### Per-source privacy invariants (non-negotiable)

- Every capture source is **opt-in** at the per-source level. Default install: clipboard + shell + notes on (low-sensitivity, text-only); screenshot/browser/audio/calendar off until toggled.
- Every captured memory carries a `source` field surfaced in the UI ("from screenshot 2026-05-09 14:32").
- "Forget this source" is one click and irreversibly deletes captures for that source.
- No capture source ever transmits off-device. Audit by `tracemind capture audit-network` which greps `traces.jsonl` for any outbound URL.

**Exit criteria for seed:** CAP-1, CAP-2, CAP-4, CAP-5 shipped; at least 3 sources live per DP; capture coverage shows up in W-8 demo. *(2026-05-11: CAP-1/2/4/5 ✅; CAP-3 deferred to Q4 alongside CAP-10 cross-modal — visual capture is not on the seed critical path.)*

---

## P1c — Performance gate (the wedge collapses without it)

Promoted from "Evaluation infrastructure" on user feedback (2026-05-11): *if init / indexing / query is slow once auto-capture is live, users churn before they feel the magic.* Performance is a product feature, not a footnote.

- [ ] **PERF-1 Performance baseline harness** — `cargo bench --bench perf_baseline` measures: cold-start (process spawn → first MCP response), query p50/p95 (under no load / under 100 events/min capture / under 1000 events/min capture), indexing throughput (events ingested per minute sustained for 10 min), idle RAM, Tier-1 hot-query latency. Output `bench/perf-{date}.json`.
- [ ] **PERF-2 CI regression gate** — `.github/workflows/perf.yml` runs PERF-1 on every PR; diff vs. main baseline; > 10% regression on any metric blocks merge. Comment posted to PR with the deltas table.
- [ ] **PERF-3 HNSW vector index (pulled forward to Q2)** — was Q4. Flat scan over SQLite caps out at ~50k entities; auto-capture pushes us past that in week 2. Use `hnsw_rs` or hand-roll; persist to `~/.tracemind/vector_hnsw.bin`.
- [ ] **PERF-4 Batched embedding + async indexing** — capture pipeline batches embeds in groups of 16 (or 100ms window, whichever first); ingestion writes via a tokio bounded channel so the capture daemon never blocks. Capture latency cap: 5ms p99 (consumer-side); ingestion latency is allowed to lag.
- [ ] **PERF-5 Lazy ColBERT rerank** — current path always runs MaxSim. Skip rerank when arm 0 (narrow) wins or top-1 vector score > 0.9. Saves ~150ms p50 on the fast path.
- [ ] **PERF-6 Cold-start audit** — `cargo flamegraph` on `tm-mcp` startup. Likely culprits: tokenizer init, ONNX session warmup, SQLite WAL replay. Target ≤ 1.5s p95 → ≤ 1.0s by Q4.
- [ ] **PERF-7 Capture-load query stress test** — spawn 1000 events/min synthetic capture load while running a 100-query benchmark. Verify p50 ≤ 500ms (Tier-0) / ≤ 800ms (Tier-1). Required for DP onboarding (DP-5 gates on this passing).

**Exit criteria for seed:** PERF-1, PERF-2, PERF-3, PERF-4, PERF-7 shipped; CI gate enforced; Q2 performance dashboard row green.

---

## P1d — Adaptive cross-context (the misfeature-fix is now active, not passive)

Promoted from passive isolation (Sprint C-0) to active learning on user feedback (2026-05-11): *the system should be smart, not rigid — feedback teaches when bridges are wrong and when the user is in the wrong context altogether.*

- [ ] **CTX-1 Learned bridge threshold per (source_ctx, target_ctx)** — replace fixed C-0 penalty with `~/.tracemind/cross_ctx_thresholds.json`: each pair has a learned threshold initialised at 0.85, decremented by 0.05 on `cross_context_bridge` negative feedback, capped above 0.5. Bridges only fire when retrieval score exceeds the threshold.
- [ ] **CTX-2 Context-mismatch detector** — after a query, if (top-1 vector score in active context) < 0.4 AND (top-1 score in *any other* context) > 0.7, emit a `ContextSuggestion { suggested_context, confidence }` in the response payload. MCP tool returns it; Tauri brief surfaces it as "this might belong in your *work* context — switch?"
- [ ] **CTX-3 Bridge labels in UI/MCP** — every bridged result carries `bridge_from: ContextId`. MCP response shows `[from: personal]`. Tauri brief shows a chip. Without visible labels, bridges feel like blur, not bridges.
- [ ] **CTX-4 Feedback-trained context routing** — `memory_feedback {kind: "wrong_context_suggestion"}` available when the system suggests the wrong switch. Trains a per-(content_topic, context) routing classifier (logistic) that biases future suggestions.
- [ ] **CTX-5 Context-aware bench** — 100 labeled pairs covering: 30 correct-isolation, 30 correct-bridge, 30 correct-switch-suggestion, 10 adversarial. Score: TP − FP. Target ≥ 85% Q2 → ≥ 92% Q4. Powers W-2.

**Exit criteria:** CTX-1, CTX-2, CTX-3 shipped; CTX-5 bench live; W-2 score ≥ 85% by Q2-end.

---

## P2 — A defensible quality number

The deck currently confesses LoCoMo F1 25.7 vs. a target of 85. That kills the meeting. Either hit a defensible number or do not show one.

- [ ] **Q-1 Tier-1 as shipping default** — auto-download Qwen 2.5 1.5B Q4 on first query, progress UI in CLI + Tauri, no `--features` flag. Tier-0 stays as fallback only.
- [ ] **Q-2 LoCoMo Tier-0 remaining fixes** — heuristic NER for span extraction (dates, money, named entities; est. +8–15 F1), yes/no oracle (est. +5–10 F1), recency bias for duplicate-entity turns (est. +3–5 F1).
- [ ] **Q-3 Query rewriting with Tier-1** — paraphrase expansion (3 variants → RRA fusion). Est. +5–8 F1 on multi-hop.
- [ ] **Q-4 LoCoMo mini-set Tier-1 run** — target ≥ 60 F1. Below 60 → keep iterating; do not put a number on the deck.
- [ ] **Q-5 Full LoCoMo run (~7000 q)** — only once mini-set hits 60. This is the headline metric.
- [ ] **Q-6 Replace deck honest-disclosure slide** — swap "F1 25.7 vs 85" for "F1 X on N=7000 LoCoMo, beats Mem0 by Y" *or* cut the slide entirely. No middle ground.

**Exit criteria:** Q-4 ≥ 60. Q-5 only after Q-4.

---

## P3 — Pitch surface rewrite (only after P0/P1 produce artifacts)

The deck is rewriting itself once P0 and P1 land. Order matters — do not rewrite slides before you have the testimonial and the head-to-head.

- [ ] **PR-1 Slide 1 is DP-6** — user's face, user's pain, user's quote. Not architecture.
- [ ] **PR-2 Slide 2 is W-5** — the 4-pane head-to-head video (persistence → context isolation → retraction). Embedded, autoplay, no narration needed.
- [ ] **PR-3 Slide 3 is the W-3 / W-2 / W-4 numbers + Q-5 number** — persistence ≥ 90%, context-isolation ≥ 95%, retraction ≥ 80%, LoCoMo F1 ≥ 65. Large type, no apology.
- [ ] **PR-4 Slide 4 is the wedge sentence (W-7)** — one line, full slide.
- [ ] **PR-5 Founder slide** — currently missing. Who are you, what have you built, why are you the person to build this. One slide.
- [ ] **PR-6 90-day forward plan slide** — replaces every honest-self-critique slide. "By August: 10 design partners, W2 ≥ 50%, LoCoMo F1 70, Engram private beta with 3 design partners."
- [ ] **PR-7 Move three-product close to appendix** — TraceMind alone is the seed pitch. Engram and Rosetta are "what the engine unlocks," not co-equal products. Cut from main flow.
- [ ] **PR-8 Drop "system of intents" from the top-line** — keep as a technical-appendix frame. Top-line is W-5.
- [ ] **PR-9 Ask sized to stage** — $500k–$1M pre-seed, not $3M seed. Right round for the artifact set.

**Exit criteria:** PR-1..PR-9 reflected in a single 9-slide deck, ≤ 4 minutes to walk through.

---

## P4 — Feedback-driven self-improvement (the architectural moat slide)

The one architectural claim that cloud competitors *cannot copy without uploading the user's corrections corpus*. Worth a slide once it's real. Until then, it's vapor — keep building.

- [ ] **F-2 Corpus extractor** — `tm-eval::corpus` reads `traces.jsonl × {positive_signals, negative_signals × recent.jsonl}` into `EvalCorpus`. Dedup on `(query_text, ts_bucket)`. Idempotent test.
- [ ] **F-3 Parameter surface (`TuneConfig`)** — move ~20 named knobs into `tm-types::TuneConfig`, load from `~/.tracemind/configs/active.toml`. CLI `tracemind config {show, edit, rollback, diff <id>}`.
- [ ] **F-4 `tm-eval` crate** — replays `EvalCorpus` against a `TuneConfig`, returns `EvalReport { f1, em, engagement_reward, neg_signal_rate, latency_p50, latency_p95 }`. Wire LoCoMo as one corpus.
- [ ] **F-5 `tm-tune` crate (GEPA-lite, on-device)** — idle-time pareto frontier per query class. CLI `tracemind tune {run, frontier, promote <id>}`. Auto-promote off by default.
- [ ] **F-6 MIPRO-style demo selection (Tier-1)** — extend mutator with demo-swap actions for Tier-1 prompts. Joint search over (instruction, demos). Blocked on Q-1.

**Exit criteria:** one design partner's `EvalReport` improves measurably over a week of their own feedback. That graph is a slide.

---

## P5 — Engine polish that visibly helps retention

Only items that a design partner would *notice* in week 2. Everything else moves to P8+.

- [ ] **C-0.9 Brief + UI surfacing of active context** — brief header shows active context; per-row context tag in Tauri brief view.
- [ ] **C-0.11 Schema-migration test** — legacy DB → migrated DB with NULL context_ids. Closes Sprint C-0.
- [ ] **UI-7 60–90s screen recording of full Tauri flow** — human capture session. Used in onboarding kit (DP-2) and pitch.
- [ ] **WorkingMemory ring buffer** — `tm-types::WorkingMemory` feeds session context into follow-up queries. Visible: "remembers what we were just talking about."
- [ ] **NarrativeResponse** — replace raw `RetrievalResult` / `AnswerResponse` at user-facing boundaries with `{ text, citations, related_threads, surprise }`.
- [ ] **First-run onboarding in Tauri** — sample data → meaningful brief in 60 seconds. Gates DP-2.

---

## P6 — Wire-up debt (only if a partner hits it)

Drop everything in this section unless a design partner files it as a bug. Do not pre-build.

- [ ] Schema-constraint + temporal-overlap contradiction triggers (TMS already has cosine)
- [ ] Persist TMS state across restarts (currently rebuilt from live triples)
- [ ] `belief_revisions` table for explicit retraction provenance
- [ ] Time-machine queries in CLI + MCP: "what was I thinking in March?"
- [ ] `GraphStore::diff(from, to)` for change inspection
- [ ] `NeedDetector` / `SentimentScorer` / `ActionMatcher` in tm-capture

---

## P7 — Tauri surfaces a partner has asked for

Each item below stays in `[ ]` until a partner names it. Do not build speculatively.

- [ ] Commitment timeline (vertical, color-coded, drawer on click)
- [ ] Intent arc visualization
- [ ] Memory garden (force-directed entity graph)
- [ ] Capture timeline
- [ ] "What I noticed" surprise panel
- [ ] Settings panel (personality, voice, brief schedule)
- [ ] Calibration panel (Brier score, pattern stats)

---

## P8 — Post-seed commitments (Q4 2026 → Q1 2027 per PROJECT_2026.md)

These are *not* indefinitely deferred — PROJECT_2026.md commits them in the quarterly roadmap. They unlock only after the seed gate clears (P0–P3 produce artifacts) but they are scheduled, not optional. Listed here in execution order:

### Q4 2026 — second surface + first revenue

- [ ] **B-1 Engram public release** — finalize `Belief` trait API, `tm-engram` full implementation, MCP tools (memory_believe, memory_retract, memory_world_at, memory_contradictions), standalone Engram MCP server, Python + TypeScript wrappers, integration test with Claude Code, publish to crates.io, benchmarks + 3 example agents. Open-core; paid commercial license.
- [ ] **B-2 First Engram commercial deal** — target $X00–$2k/year/dev at one regulated org (HIPAA / finance / defense). PROJECT_2026.md §10 Q4 metric.
- [ ] **Q-7 World model v1** — `f_topic` MLP (2-layer InfoNCE 384→512→384) for L1 silent prefetch, Platt scaling / isotonic regression on `f_outcome`, extend LinUCB context vector. PROJECT_2026.md §3 bet #7.
- [ ] **Q-9 Iterative / agentic retrieval** — multi-step query refinement (Self-RAG / IR-CoT). +10 F1 on multi-hop. PROJECT_2026.md §3 bet #6.
- [ ] **Q-10 Cross-modal entity-edge join** — bind capture pipelines from P1b (screenshots / audio / code) into shared entity graph via `ModalIngestPipeline` co-occurrence edges, arm 5 in tm-controller, cross-modal chains + citations. Pulled forward from Q1 2027 because capture inputs ship Q2-Q3.

(Note: HNSW moved to P1c PERF-3, Q2. Was originally P8 Q4 but auto-capture volume forces it earlier.)

### Q1 2027 — Series A prep

- [ ] **B-3 TraceMind Pro alpha** — $5–10/mo for E2EE multi-device sync via user's own iCloud/Drive. Never our servers. PROJECT_2026.md §10 row 2.
- [ ] **B-4 First enterprise pilot signed** — 5-figure deal, single regulated org buying seats. PROJECT_2026.md §10 row 3.
- [ ] **B-5 6-month retention cohort data** — charts ready for Series A meetings.
- [ ] **Q-11 Cross-modal expansion** — VS Code / Cursor / JetBrains deep IDE plugins, audio-conversation summarization, mail/calendar bidirectional sync.

---

## P9 — Business + ops commitments (Q2 → Q4 per PROJECT_2026.md)

Non-engineering tasks the founder owns. Tracked here so they don't fall off the radar.

### Hiring

- [ ] **H-1 Hire #1 — ML engineer** (Q3 2026). Owns LoCoMo, bandit, world model. Non-negotiable per PROJECT_2026.md §9 + risk #6. Trigger: seed term sheet.
- [ ] **H-2 Hire #2 — Design / product** (Q4 2026). Owns Tauri UX + onboarding + brand. Trigger: term sheet closes + Engram launch traction.

### Funding

- [ ] **FN-1 Friends & family / angel (optional)** — $100–250k Q2 2026 only if runway forces it. PROJECT_2026.md §10.
- [ ] **FN-2 Pre-seed / seed close** — $1–1.5M Q3-Q4 2026. Trigger: P0–P3 artifacts complete (DP-6 + W2 ≥ 40% + W-5 video + LoCoMo ≥ 60).
- [ ] **FN-3 Investor list assembled** — local-first / privacy-aligned funds and angels (Linear / Cursor / Vercel / Obsidian backers). Avoid generic AI Tier-2 funds that push SaaS economics. PROJECT_2026.md §10.

### Launch + press

- [ ] **L-1 Hacker News launch (Q3 2026)** — "Why every AI memory layer forgets across sessions, blurs your contexts, and contradicts itself — and what we built instead." Three numbers (persistence, context isolation, retraction) + open repo. PROJECT_2026.md §5 distribution bet #2.
- [ ] **L-2 MCP host directory listings** — Anthropic MCP servers list + Goose extensions registry + Cline marketplace. Free organic distribution. PROJECT_2026.md §7 metric row 7 (target: 4 by Q3, 6 by Q4).

### Cadence

- [ ] **OPS-1 Daily standup written to `~/.tracemind/standup.md`** — dogfood our own product. PROJECT_2026.md §11.
- [ ] **OPS-2 Weekly Friday review** — 3 numbers: design partners, W2, F1. Update PROJECT_2026.md when any quarterly milestone slips. PROJECT_2026.md §11.
- [ ] **OPS-3 Monthly DP check-ins (15 min each) + risk register review + deck refresh**.
- [ ] **OPS-4 Quarterly re-read of PROJECT_2026.md** + Q+1 milestone update.

---

## P10 — Indefinitely deferred (kill or revisit if a partner asks)

Excellent engineering. Not scheduled in PROJECT_2026.md. Park.

(Note: clipboard, shell, screenshot, audio, browser, calendar capture **promoted to P1b** as seed-critical. Only TTS playback + Obsidian-style passive vault sync remain here.)

### Auxiliary capture / output (deferred)

- [ ] Piper TTS brief playback (audio *output*, not input — distinct from CAP-6 Whisper-tiny input)
- [ ] Obsidian vault import (deferred until a DP asks; CAP-* already covers the primary capture surfaces)

### Nightly processes (deferred)

- [ ] `tm-reflect` cron, nightly TMS check, deductive/inductive promotion, temporal GC, two-speed ingestion

### Rosetta foundation (deferred — re-open after Engram has a commercial deal)

- [ ] `tm-semcode`, intent extraction pipeline, `SemanticDiff`, intent drift
- [ ] `tm-rosetta` CLI, MCP tools, VS Code extension stub

### Mobile + sync (deferred to post-Series A unless a partner forces it)

- [ ] UniFFI bindings, iOS app, Android app
- [ ] Apple FoundationModels backend, photo ingest
- [ ] `tm-sync` (Automerge CRDTs), E2E encryption, opt-in encrypted-cloud tier
- (Note: TraceMind Pro E2EE sync above (B-3) is the *consumer* sync product; UniFFI iOS/Android is the *separate* native-app bet, deferred until Pro proves out.)

### Scale + world model v2 (deferred to 2027+)

- [ ] `f_outcome` v2 transformer, Mamba history compression
- [ ] GraphSAGE for AnalogySolver, Louvain/Leiden in Consolidator
- [ ] Factorization machine for PatternDetector, `tm-preference` crate
- [ ] Counterfactual replay, context-budget allocator

(Note: HNSW moved to P1c PERF-3; iterative retrieval moved to P8 Q4 Q-9.)

### LLM packaging + on-device finetune (deferred — see PROJECT_2026.md §3 weak-spot row "no personalization")

- [ ] `tm-llm` crate (ModelManifest + ModelRegistry, first-run fetch, LoRA adapter slot)
- [ ] Python sidecar QLoRA pipeline, 3 default LoRA roles, nightly opt-in schedule
- Flagged as Series A blocker in PROJECT_2026.md, but not before. Revisit Q1 2027.

---

## Evaluation infrastructure (driven by what the pitch needs)

- [ ] Q-5 above (full LoCoMo, ≥ 65 F1)
- [ ] W-3 above (persistence microbench)
- [ ] W-2 above (context-isolation microbench)
- [ ] W-4 above (retraction microbench)
- [ ] New crate `tm-bench-memory` to house all three (persistence + context-isolation + retraction) under one CLI. Public leaderboard at `tracemind.dev/memory-bench` (PROJECT_2026.md §6 Q3).
- [ ] Performance benchmarking (idle RAM, active RAM, cold-query, Tier-1 hot-query latency) — needed for Q-6 disclosures + PROJECT_2026.md technical-credibility dashboard
- [ ] Cross-modal eval harness — deferred to post-seed (Q1 2027 per PROJECT_2026.md)
- [ ] Bitemporal correctness test suite — deferred
- [ ] Intent preservation eval — deferred

---

## Operating rules

1. **No work in P4+ until P0 has at least 3 active design partners.** Building the moat is irrelevant if no one is around to be locked in. P8 (post-seed) and P9 (business / ops) work is unblocked only when the seed gate clears.
2. **No deck rewrite until P0 produces DP-6 and P1 produces W-5.** Slides without artifacts are vapor.
3. **No three-product narrative in any external comms until TraceMind alone hits W2 ≥ 40%.** Optionality reads as lack of conviction.
4. **Weekly review every Friday.** Three numbers: design partners onboarded, W2 retention, LoCoMo F1. Anything else is noise.
5. **If a P0 candidate refuses three times, the wedge is wrong.** Stop building, run brainstorming, return.
6. **Two surfaces, both P0.** MCP is the wedge (host integration → design-partner channel); Tauri is the flagship (where users go to *see* their memory). Investing only in one collapses the company narrative — into a plugin business if MCP-only, into a brand-from-zero problem if Tauri-only. Lead the deck with MCP-in-Claude-Code for the demo moment; slide 2 is the Tauri walkthrough.
7. **Project 2026 (`docs/PROJECT_2026.md`) is the canonical strategy doc.** TASKS.md operationalises it. If TASKS.md drifts from PROJECT_2026.md, update both — don't fork.
