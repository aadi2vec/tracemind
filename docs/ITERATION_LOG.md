# TraceMind Iteration Log

A running ledger of problems found, root causes, and fixes applied while
hardening TraceMind toward "best product" quality. Newest entries at the
top. Each entry is self-contained so it survives context compaction.

Format per entry:

- **Problem** — the observed symptom.
- **Root cause** — why it actually happened (not just the symptom).
- **Fix** — what changed, with file paths.
- **Verification** — how we proved it works.
- **Status** — `FIXED` / `IN PROGRESS` / `DEFERRED` (with reason).

---

## 2026-07-25

### Ambient web capture required a bookmarklet — nobody uses that
- **Problem** — the only real ingestion path that worked was typed-in text
  (`tracemind ingest "..."`) and a manual browser *bookmarklet* the user had
  to click on every page. The named modality sources people actually expect
  (Safari, Chrome, Notes, screenshots) either watched synthetic drop folders
  (`~/.tracemind/web-pins`, `~/.tracemind/email`) that nothing writes to, or
  produced no text. The product looked alive in demos only because a human
  was typing.
- **Root cause** — `WebPinSource` watches a pin-drop folder fed by a browser
  extension that doesn't exist; `browser_capture.rs` is a click-to-capture
  bookmarklet (manual, not ambient). No source read the browser's own
  history. So "ambient capture" wasn't ambient.
- **Fix** — new `tm-capture/src/browser_history.rs`: `BrowserHistorySource`
  reads Chrome and Safari history **SQLite DBs directly** (copy to a temp
  file to dodge the live WAL lock, open read-only). Per-browser high-water
  cursor on `visit_time`; first run backfills the last 7 days (capped/
  paginated at 50 visits per poll) then only emits new visits. Non-web
  schemes (`chrome://`, `about:`, `file:`) dropped. URL emitted first so
  `ingest_fast` promotes it Tier-1. Wired into `ModalityRegistry::
  with_defaults`, mapped `"browser-history" → CaptureSource::Browser` (one
  consent toggle covers history + the legacy bookmarklet).
- **Verification** — 5 new unit tests (epoch round-trips for both browsers,
  Chrome+Safari schema reads, `chrome://` filtering, `poll` cursor
  pagination). Live: `capture enable browser` + daemon against the real
  Chrome DB captured actual visited pages (FIFA World Cup ticket pages) at
  Tier-1; `query "world cup tickets"` then recalled the FIFA URL with real
  BGE embeddings. No extension, no typing.
- **Note / known limits** — Safari's `~/Library/Safari/History.db` is
  TCC-protected: reads return `Operation not permitted` until the user grants
  the binary **Full Disk Access** (the source degrades gracefully — logs at
  debug, emits nothing). Chrome needs no special permission. URL entities are
  still mis-typed by the heuristic NER (a captured URL becomes an
  `Organization`) — a follow-up NER-quality item, not a capture bug.
- **Status** — FIXED (Chrome ambient); Safari gated on Full Disk Access.

### Demo ran on `--hash-embed`, hiding real retrieval quality
- **Problem** — the Tauri UI demo bridge and the screen-recording demo
  script forced `--hash-embed` on every CLI call, so the whole demo used
  semantically meaningless hash embeddings (LoCoMo F1 ~48 vs ~70) — the
  opposite of what `demo/README.md` promises ("real BGE … No --hash-embed").
- **Root cause** — `demo-bridge.mjs` spawned `[CLI, "--hash-embed", ...args]`
  and `demo/capture-screen.sh` set `TM="…/tracemind --hash-embed"`, leftovers
  from when embeddings were deterministic-only for CI.
- **Fix** — removed `--hash-embed` from both user-facing demo paths so they
  exercise the real BGE-small ONNX embedder the product ships with. CI test
  harnesses (`demo_smoke.sh`, `e2e-test.sh`, `run_exit_gate.sh`) keep it —
  determinism there is legitimate.
- **Verification** — restarted the demo bridge on real BGE; ingest extracted
  `Alice` / `Mercury pricing project` semantically and a follow-up query
  retrieved across documents.
- **Status** — FIXED.

## 2026-07-24

### Notes NER merged heading into the next line's name ("Standup Jane")
- **Problem** — ingesting a note `# Standup\n\nJane owns the pricing rollout`
  produced a single `[Person] Standup Jane` entity instead of a `Standup`
  heading and a standalone `Jane`.
- **Root cause** — `extract_entities` in `tm-ingest/src/pipeline.rs` tokenizes
  with `text.split_whitespace()`, which collapses newlines. The Title-Case
  run scanner then merged the heading word "Standup" with the body's first
  Title-Case word "Jane" across the (now invisible) line break.
- **Fix** — build the flat word list per-line and record `line_start` flat
  indices; the multi-word merge loop now `break`s when it would cross a line
  start, so Title-Case runs can't span a line break.
- **Verification** — new unit test
  `test_title_case_run_does_not_cross_newline` (asserts no "Standup"+"Jane"
  merge, and a standalone "Jane" survives). Live demo: the note now yields
  separate `Standup` / `Jane` entities.
- **Status** — FIXED.

### Local file path + markdown `#` leaked into notes answers
- **Problem** — querying a captured note returned an extractive answer whose
  first line was the note's absolute file path
  (`/…/Obsidian/standup.md`) followed by a raw `# Standup` heading.
- **Root cause** — `NotesVaultSource` reuses `Modality::Web`, and
  `WebPreprocessor` (a) unconditionally prepends `payload.uri` to the
  canonical text — fine for a real URL, noise for a local path — and (b)
  embeds the raw markdown, so `#` glyphs survive into answers.
- **Fix** —
  - `tm-ingest/src/multimodal.rs`: `WebPreprocessor` only prepends the URI
    when it is `http(s)://`; local paths are dropped from canonical (blob
    still preserves provenance).
  - `tm-capture/src/modalities.rs`: `NotesVaultSource` emits canonical text
    with leading markdown heading markers stripped per line; raw bytes are
    still stored as the provenance blob.
- **Verification** — live demo: "who owns the pricing rollout" now answers
  `Standup / Jane owns the pricing rollout.` (no path, no `#`), and "where
  do we meet about the enterprise tier" returns the email line naming
  "Room 3".
- **Status** — FIXED.

### Daemon ignored `--hash-embed`, silently breaking retrieval
- **Problem** — passing `--hash-embed` to `tracemind-capture` did nothing.
  The daemon ingested with real BGE while a `--hash-embed` query CLI read
  from a hash space → mismatched vector spaces → "I have nothing stored
  about that" even though signals were present.
- **Root cause** — `CaptureConfig::from_env` only checked the
  `TM_HASH_EMBED=1` env var; it never inspected `std::env::args()`, so the
  CLI flag that the `tracemind` binary honors was a no-op for the daemon.
- **Fix** — `tm-capture/src/main.rs`: `hash_embed` now ORs the env var with
  `std::env::args().any(|a| a == "--hash-embed")`, so the flag is spelled
  the same way for daemon and CLI.
- **Verification** — daemon logged `BGE-load-count: 0` when the flag was
  passed (previously it loaded BGE regardless). Live BGE (no flag) demo
  below confirms recall works when both sides agree on the embedder.
- **Status** — FIXED.

### Modality captures invisible in `tracemind recent`
- **Problem** — ambient modality captures (email, notes, calendar, …) never
  appeared in `tracemind recent`; only clipboard/shell did.
- **Root cause** — `multimodal_loop` never wrote to `RecentStore`, unlike
  `clipboard_loop`/`history_loop`.
- **Fix** — `tm-capture/src/main.rs`: open `RecentStore` in `multimodal_loop`
  and call `record_capture` after each successful ingest, with a display
  string that falls back text → hint → `[<modality> capture]`.
- **Verification** — live demo (`HOME=/tmp/tm-demo-x`): after dropping an
  `.eml` and an Obsidian `.md`, `recent` shows
  `[notes-vault] t1 promoted # Standup  Jane owns the pricing rollout…` and
  `[imap-inbox] t2 promoted Subject: Q3 pricing decision…`. Cross-source BGE
  recall then answered "who owns the pricing rollout" → "Jane owns the
  pricing rollout" while surfacing "Room 3" from the email.
- **Status** — FIXED.

### CAP-1 gating for the multimodal capture loop
- **Problem** — `multimodal_loop` in `tm-capture` started all 8 ambient
  modality sources (Notes, Calendar, Mail, Photos, PDF, screenshots,
  voice, browser) unconditionally, with no per-source consent check —
  unlike `clipboard_loop`/`history_loop`/`browser_capture_loop`, which
  fail closed behind `load_enabled(...)`.
- **Root cause** — the loop was wired to `ModalityRegistry::with_defaults`,
  and 3 modality sources (PDF/Email/Photo) had no `CaptureSource` enum
  variant, so they *could not* be gated even in principle.
- **Fix** —
  - `tm-types/src/capture_permissions.rs`: added `CaptureSource::{Pdf,
    Email, Photo}` (+ `as_str`/`parse`/`all()`/`description`); all
    default-off. `load_or_default` backfill gives existing installs the
    new toggles as disabled.
  - `tm-capture/src/modalities.rs`: added `capture_source_for(name)`
    mapping + `ModalityRegistry::with_enabled(data_dir, &perms)` which
    drops any source not explicitly enabled *before its first poll*.
  - `tm-capture/src/main.rs`: `multimodal_loop` now loads permissions and
    builds via `with_enabled`; malformed perms → zero sources; empty set
    → loop doesn't start.
- **Verification** — 15 tm-capture + 9 tm-types tests pass, incl. new
  `with_enabled_keeps_only_enabled_sources` and
  `every_default_source_maps_to_a_permission`. Live daemon demo: fresh
  install polls only `notes-vault`; after `capture enable calendar email`
  it polls `["imap-inbox","eventkit","notes-vault"]`; a dropped `.eml`
  was auto-ingested (`ingested kind="email" source=imap-inbox`).
- **Status** — FIXED.

### Open follow-ups from the demo (being worked next)
- Modality captures don't mirror into the `RecentStore` ring buffer, so
  `tracemind recent` shows only clipboard/shell. → see next entries.
- Multimodal-ingested signals weren't surfaced by `tracemind query`
  under `--hash-embed`. → under investigation.
