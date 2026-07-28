# TraceMind — recorded demos

Two CLI walkthroughs of the local memory OS, plus a **GUI walkthrough**
of the desktop-app frontend (no CLI in frame). Each CLI demo has a
re-runnable shell script (in `scripts/`), a **plain-text transcript**
of one clean run, and a **macOS `script(1)` typescript** that
preserves timing so you can replay it with real pacing.

Everything runs on-device. No network, no telemetry, no cloud
dependencies. Real BGE-small ONNX embeddings; SQLite graph +
JTMS truth-maintenance layer; ambient capture and tiered answers
all local.

## 1. Composition wedge — `memory_context_for`

The innovation shipped in July 2026: one MCP verb every host
(Claude Code, Cursor, Goose, Windsurf) can bind to a keystroke →
a token-budgeted, cited context brief on the topic the user is
about to ask the LLM about.

| Artifact | What it is |
|---|---|
| `../../scripts/demo_context_for.sh` | Re-runnable shell script (~60-90s at default pacing) |
| `context_for.gif` | Rendered screen recording — drop into a README or slide |
| `context_for.cast` | asciicast v2 recording — `asciinema play docs/demos/context_for.cast` |
| `context_for_transcript.txt` | Clean ANSI-stripped record of one full run |
| `context_for_transcript.ansi.txt` | Same run, colors preserved (`cat` it in a terminal) |
| `context_for.typescript` | macOS `script(1)` recording — replay with `script -p` |

Scenes: cold seed of a realistic memory (URLs, decisions, a
supersession pair, a Commitment) → single `context-for` call →
budget-aware truncation → JSON payload (what the MCP host actually
receives) → honest grounding on an unknown topic.

## 2. Full product tour

End-to-end walkthrough of every product surface (~4-5 min at default
pacing). 10 acts, 26 scenes.

| Artifact | What it is |
|---|---|
| `../../scripts/demo_product_tour.sh` | Re-runnable shell script |
| `product_tour.gif` | Rendered screen recording — drop into a README or slide |
| `product_tour.cast` | asciicast v2 recording — `asciinema play docs/demos/product_tour.cast` |
| `product_tour_transcript.txt` | Clean ANSI-stripped record of one full run |
| `product_tour_transcript.ansi.txt` | Same run, colors preserved |
| `product_tour.typescript` | macOS `script(1)` recording — replay with `script -p` |

Coverage:

- **Act I — Onboarding** — cold start, `onboard --dry-run`
- **Act II — Ingestion** — text ingest, URL ingest (browser-shaped: raw URL + registrable-domain Organization), bulk directory import, ambient capture permissions surface, capture-doctor (honest permission story for Safari / screenshot OCR)
- **Act III — Memory backend** — bandit-routed retrieval (`query`), tiered answer layer (`ask`), quick-recall launcher target, immutable trace log
- **Act IV — Graph features** — typed backlinks (LM-1), contexts (decoupled-by-default namespaces, C-0), Memory Views (LM-11), pending relations pool (LM-9), Karpathy-style daily note (LM-5c)
- **Act V — Retraction beat** — JTMS supersession on conflicting typed triples; the `⚠ Retraction:` reconcile message is the wedge
- **Act VI — System of Intents** — commit an intent, daily brief, open commitments
- **Act VII — Composition wedge** — the CLI mirror of `memory_context_for` (bridge to demo 1)
- **Act VIII — Self-improvement** — bandit arm stats, `policy show` (GEPA-tuned retrieval policy), nightly on-device runs
- **Act IX — Privacy** — full graph export as Obsidian-compatible markdown bundle (LM-16), storage report
- **Act X — Wrap** — weekly digest

## 3. GUI walkthrough — the desktop app (no CLI)

The same local backend, driven through the Tauri desktop frontend instead
of the terminal. Captured by running the Vite frontend in a headless
Chromium (Playwright) against the real `tracemind` backend via the dev-only
`demo-bridge.mjs` shim, so every number on screen is real retrieval / real
traces — not mocked.

| Artifact | What it is |
|---|---|
| `gui/product_gui_tour.webm` | Full-session screen recording of the walkthrough |

> **Note (2026-07-28).** The four browser-captured stills that used to sit here
> — `01_home.png`, `02_ask.png`, `03_ingest.png`, `04_traces.png` — were
> deleted during a failed native-recording attempt and are **not recoverable**
> (`docs/demos/` is untracked by git). The `.webm` above survives. Regenerate
> the stills with the browser recipe below, or supersede them entirely with the
> native capture in §4, which covers these four surfaces and fifteen more.

**Scope note.** `demo-bridge.mjs` is a dev-only HTTP shim that backs the
browser preview from the `tracemind` CLI. It implements the primary-flow
handlers (`ingest` / `query` / `traces` / `mode` / `context`), so the four
surfaces above render faithfully in a plain browser. The advanced dev-mode
surfaces (Composer, Graph, Dashboard, Threads, Ledger, Commitments, Review,
…) call typed Tauri IPC commands that the shim does not implement — those
require the **native app** (`./target/release/tracemind-app`), which wires
all 103 IPC handlers to the real Rust backend.

Extending the shim to cover them is **not** a good option: of the ~25
read-only handlers those surfaces need, roughly ten (`cmd_graph`,
`cmd_dashboard`, `cmd_memory_garden`, `cmd_community_overlay`,
`cmd_surprising`, `cmd_entity_trends`, `cmd_next_actions`, `cmd_threads_list`,
`cmd_compose_simple`, `cmd_event_graph`) have no CLI equivalent at all. Backing
them would mean re-implementing Rust query logic against SQLite in JavaScript,
or fabricating data — and a fabricated demo is exactly what the "use the real
BGE embedder, not `--hash-embed`" comment at the top of `demo-bridge.mjs`
exists to prevent. **Record the native app instead** (§4).

## 4. GUI walkthrough — native app, all 19 surfaces

`../../scripts/demo_gui_native.sh` drives the real `tracemind-app` through
every sidebar surface and screenshots each one. Every pixel is the real Rust
backend — no shim, no mocks.

    ./scripts/demo_gui_native.sh              # 6s per surface
    DWELL=4 ./scripts/demo_gui_native.sh      # faster
    KEEP_DATA=1 ./scripts/demo_gui_native.sh  # reuse the seeded data dir

It needs **no Accessibility permission**. Rather than scripting clicks, it sets
a backend-persisted flag (`$TM_DATA_DIR/demo_tour.json`) that puts the app into
an auto-tour: the app goes fullscreen on its own and advances one surface every
`DWELL` seconds. The script only takes screenshots, which needs Screen
Recording permission.

The same tour is reachable from the UI at **Settings → Developer mode →
Auto-tour surfaces**. It is backend-persisted rather than `localStorage`-backed
(unlike `tm:dev_mode`) precisely so a recording harness outside the webview can
enable it; see the `DemoTourState` doc comment in `crates/tm-tauri/src/main.rs`.

> **⚠ The Mac must be unlocked and stay unlocked for the whole run.**
> While the screen is locked, macOS does not composite the desktop and
> `screencapture` returns **solid-black PNGs with exit code 0** — the capture
> appears to succeed and you get a directory of black frames. `caffeinate`
> does *not* help: it wakes the display, which then shows the lock screen.
> The script guards against this by failing loudly on any frame under 500 KB
> (a black 2940×1912 PNG is ~108 KB; a real one is several MB), but the only
> actual fix is to unlock the machine and disable the screen lock for the
> ~3 minutes the run takes.

**Video assembly needs a real ffmpeg, which this machine does not have.** The
run always produces the 19 stills; it assembles `product_gui_native.mp4` /
`.gif` only if `ffmpeg` is on `PATH`. The Playwright-bundled build at
`~/Library/Caches/ms-playwright/ffmpeg-*/ffmpeg-mac` **cannot** be used — it
ships only the `image2` muxer and `png` encoder (no gif, no mp4, no x264).
Homebrew is not installed either. To get video, either install Homebrew and
`brew install ffmpeg`, or drop a static build from
[evermeet.cx/ffmpeg](https://evermeet.cx/ffmpeg/) on `PATH`.

### Re-record the GUI walkthrough

    # 1. seed a throwaway data dir with the real backend
    export TM_DATA_DIR=/tmp/tm-demo-gui
    rm -rf "$TM_DATA_DIR" && mkdir -p "$TM_DATA_DIR"
    ./target/release/tracemind ingest "https://www.fifa.com/tickets FIFA World Cup 2026 Tickets"
    ./target/release/tracemind ingest "Alice works at Anthropic and Bob works at Anthropic. Carol founded Anthropic."
    # ...seed a few more captures + a commitment...

    # 2. start the dev bridge against that data dir
    cd crates/tm-tauri/ui && TM_DATA_DIR=/tmp/tm-demo-gui bun demo-bridge.mjs &

    # 3. start the Vite frontend and drive it with Playwright (headless Chromium)
    bun run dev            # serves http://localhost:3000
    python3 /tmp/tm_gui_clean.py   # navigates + screenshots + records the .webm

Convert the `.webm` to a GIF with a full ffmpeg build (the Playwright-bundled
ffmpeg has no gif muxer):

    ffmpeg -i gui/product_gui_tour.webm \
      -vf "fps=10,scale=900:-1:flags=lanczos" gui/product_gui_tour.gif

## How to run / re-record

Everything below assumes you're at the repo root and have built the
release binary at `./target/release/tracemind` (`cargo build --release -p tm-cli`).

Human-paced live run (what you'd see live in a demo):

    ./scripts/demo_context_for.sh
    ./scripts/demo_product_tour.sh

Zero-pacing CI-style dry run (fastest way to sanity-check both):

    PAUSE_SHOT=0 PAUSE_RUN=0 PAUSE_READ=0 ./scripts/demo_context_for.sh
    PAUSE_SHOT=0 PAUSE_RUN=0 PAUSE_READ=0 ./scripts/demo_product_tour.sh

Re-record the plain-text transcripts (replaces the files here):

    ./scripts/demo_context_for.sh 2>&1 > docs/demos/context_for_transcript.ansi.txt
    ./scripts/demo_product_tour.sh 2>&1 > docs/demos/product_tour_transcript.ansi.txt
    # then strip ANSI:
    sed 's/\x1b\[[0-9;]*m//g' docs/demos/context_for_transcript.ansi.txt   > docs/demos/context_for_transcript.txt
    sed 's/\x1b\[[0-9;]*m//g' docs/demos/product_tour_transcript.ansi.txt  > docs/demos/product_tour_transcript.txt

Re-record the typescripts with real timing:

    script -q docs/demos/context_for.typescript  ./scripts/demo_context_for.sh
    script -q docs/demos/product_tour.typescript ./scripts/demo_product_tour.sh
    # replay:
    script -p docs/demos/context_for.typescript

Re-record the asciicasts + GIFs (the shareable recordings checked in here).
`asciinema` records the run; `agg` renders the `.cast` to a `.gif`. On this
machine asciinema is a Python module (`~/Library/Python/3.9/bin/asciinema`)
and `agg` is a cargo binary (`~/.cargo/bin/agg`):

    asciinema rec docs/demos/context_for.cast --overwrite \
      --title "TraceMind — Composition wedge (memory_context_for)" \
      -c "PAUSE_SHOT=2 PAUSE_RUN=1 PAUSE_READ=2 ./scripts/demo_context_for.sh"
    asciinema rec docs/demos/product_tour.cast --overwrite \
      --title "TraceMind — Full product tour" \
      -c "PAUSE_SHOT=2 PAUSE_RUN=1 PAUSE_READ=2 ./scripts/demo_product_tour.sh"

    # render to GIF (monokai theme, 14px)
    agg --font-size 14 --theme monokai docs/demos/context_for.cast  docs/demos/context_for.gif
    agg --font-size 14 --theme monokai docs/demos/product_tour.cast docs/demos/product_tour.gif

    # or just replay a cast in the terminal
    asciinema play docs/demos/product_tour.cast

## Data dirs

Both scripts use throwaway `/tmp` locations so they don't touch your
real `~/.tracemind/`:

- context-for demo → `/tmp/tm-demo-context-for/`
- product tour    → `/tmp/tm-demo-tour/`
- product tour import fixture → `/tmp/tm-demo-tour-import/`
- product tour export dir     → `/tmp/tm-demo-tour-export/`

Each run wipes and recreates these; no state leaks between runs.
