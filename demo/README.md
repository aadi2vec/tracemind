# TraceMind walkthrough

Real content, real BGE-small ONNX embeddings, real GLiNER NER, real on-device retrieval. No fabricated narrative, no `--hash-embed`. State lives in `/tmp/tm-walkthrough` so your real `~/.tracemind/` is untouched.

## Files

- **`walkthrough.sh`** — the 5-act presentation. Self-contained, paced for narration.
- **`record.sh`** — wraps `walkthrough.sh` with `screencapture -V` to produce a `.mov`.
- **`capture-screen.sh`** + **`ocr.swift`** — Vision-framework OCR wrapper for the ambient-capture story (optional).
- **`recordings/`** — output: `.mov` videos and `.txt` transcripts.

## Quickstart

```bash
# build (one-time, ~1 min)
cargo build --release --workspace --exclude tm-tauri

# run the walkthrough
./demo/walkthrough.sh

# reset state between rehearsals
rm -rf /tmp/tm-walkthrough
```

## Pacing

- `DEMO_PAUSE=1.2 ./demo/walkthrough.sh` — default; ~3 min, comfortable for narration
- `DEMO_PAUSE=2.0 ./demo/walkthrough.sh` — slower, give Act 3 (world model) more breathing room
- `DEMO_PAUSE=0.0 ./demo/walkthrough.sh` — smoke test (~30s)

## Recording video

### Option A — built-in macOS recorder (no setup, recommended)

1. `Cmd+Shift+5` → choose "Record Selected Portion" or "Record Entire Screen"
2. Click **Record**
3. In your terminal, run `./demo/walkthrough.sh`
4. When done, click ⏹ in the menu bar
5. Recording lands on Desktop as `Screen Recording <timestamp>.mov`

### Option B — CLI wrapper (needs Screen Recording permission)

1. Grant permission: System Settings → Privacy & Security → **Screen & System Audio Recording** → enable for your terminal app (Terminal / iTerm / VS Code / etc.)
2. Run `./demo/record.sh`
3. Output lands at `demo/recordings/walkthrough-<timestamp>.mov`

If `record.sh` produces no file, the parent process doesn't have Screen Recording permission. Use Option A.

### Option C — terminal-only transcript (text + ANSI colors)

```bash
script -q demo/recordings/walkthrough.txt ./demo/walkthrough.sh
```

Replays in any terminal with `cat demo/recordings/walkthrough.txt`. Good for diff'ing runs or sharing async, not for an investor call.

## Story arc

| Act | What's shown | Real components exercised |
|-----|--------------|---------------------------|
| 1 | Memory ingest + query | BGE-small (384-dim) · GLiNER NER · SQLite KG · ColBERT rerank · 1-hop expansion |
| 2 | Commitment primitive | `tm-intent` state machine · `tm-reflect` brief renderer |
| 3 | World model bootstrap | `tm-world-model` logistic regression · auto-retrain-on-resolve · preflight |
| 4 | Calibration & trust | Brier score · reliability table · auto-quiet on drift |
| 5 | MCP wire-up | `tm-mcp` JSON-RPC server · 21 tools |

If a single act is enough:
- 60-second pitch → Act 3 (world model preflight is the "magic moment")
- "Show me the system" → Acts 1 + 3
- Technical depth → all five

## What the walkthrough does **not** do

- Does **not** require Tier-1 LLM weights — `ask` falls back to Tier-0 extractive with an honest "pull weights for prose" hint
- Does **not** demonstrate ambient capture (clipboard / screen OCR) — see `capture-screen.sh` for that path; it's a different demo
- Does **not** populate the calibration table with non-zero numbers — auto-retrain-on-resolve resets `trained_at`, so calibration honestly says "warming up" right after the bootstrap. This is real product behavior, not a bug.
