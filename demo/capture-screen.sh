#!/usr/bin/env bash
# capture-screen.sh — TraceMind screen-grab → OCR → ingest helper.
#
# Workflow:
#   1. Interactive screen selection via macOS `screencapture -i` (drag to select)
#   2. OCR via Apple Vision framework (demo/ocr.swift) — built-in, no models
#   3. Pipe recognized text into `tracemind ingest -`
#
# All local. Same OCR engine as macOS Live Text. macOS 11+. No deps.
#
# Usage:
#   ./demo/capture-screen.sh                   # interactive selection
#   ./demo/capture-screen.sh --full            # full screen
#   ./demo/capture-screen.sh path/to/image.png # OCR an existing image
#
# Env:
#   TM_DATA_DIR   — passed through to tracemind (default ~/.tracemind)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Real BGE-small ONNX embeddings — the demo must show real semantic
# retrieval, never the deterministic-but-meaningless --hash-embed path.
TM="${ROOT}/target/release/tracemind"
OCR="${ROOT}/demo/ocr.swift"

if [[ ! -x "${ROOT}/target/release/tracemind" ]]; then
  echo "Build first: cargo build --release --bin tracemind" >&2
  exit 1
fi
if [[ ! -f "$OCR" ]]; then
  echo "Missing $OCR" >&2
  exit 1
fi

mode="${1:-interactive}"
img=$(mktemp -t tm-capture-XXXX).png

cleanup() { rm -f "$img"; }
trap cleanup EXIT

case "$mode" in
  --full)
    echo "▸ capturing full screen..."
    screencapture -x "$img"
    ;;
  -h|--help)
    sed -n '2,16p' "$0"; exit 0
    ;;
  --*)
    echo "unknown flag: $mode" >&2; exit 2
    ;;
  "" | interactive)
    echo "▸ drag-select the area to capture (Esc to cancel)..."
    # -i interactive, -x silent shutter, -t png. Returns non-zero if cancelled.
    screencapture -i -x -t png "$img"
    if [[ ! -s "$img" ]]; then
      echo "  cancelled."; exit 0
    fi
    ;;
  *)
    if [[ -f "$mode" ]]; then
      cp "$mode" "$img"
    else
      echo "no such image: $mode" >&2; exit 2
    fi
    ;;
esac

echo "▸ OCR via Apple Vision..."
text=$(swift "$OCR" "$img" 2>/dev/null || true)

if [[ -z "${text// }" ]]; then
  echo "  no text recognized."
  exit 0
fi

printf '%s\n' "── recognized ─────────────────────────────────"
printf '%s\n' "$text"
printf '%s\n' "───────────────────────────────────────────────"

echo "▸ ingesting into TraceMind..."
printf '%s\n' "$text" | $TM ingest -
