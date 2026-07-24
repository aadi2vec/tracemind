#!/usr/bin/env bash
#
# TM-NLP-005 — pre-populate the HF cache layout that `hf-hub` and `fastembed`
# expect, so that a shipped binary can run fully offline against bundled
# weights. Invoked by Tauri's `beforeBundleCommand` during `.dmg` / `.app`
# packaging; can also be run by hand to seed `~/.tracemind/models/`.
#
# Usage:
#   scripts/fetch-models.sh <output_dir>
#
# Layout produced at <output_dir>:
#   models--onnx-community--gliner_small-v2.1/blobs/<sha>
#   models--onnx-community--gliner_small-v2.1/snapshots/<rev>/onnx/model_quantized.onnx
#   models--onnx-community--gliner_small-v2.1/snapshots/<rev>/tokenizer.json
#   models--onnx-community--gliner_small-v2.1/snapshots/<rev>/config.json
#   models--onnx-community--gliner_small-v2.1/refs/main
#   models--mixedbread-ai--mxbai-edge-colbert-v0-17m/...
#   models--Xenova--bge-small-en-v1.5/...
#
# The directory can then be passed to binaries via:
#   - Tauri `bundle.resources` (placed at Contents/Resources/models/)
#   - $TM_MODELS_DIR for air-gapped deployments
#   - copied to ~/.tracemind/models/ for a local dev seed
#
# Requirements: bash, curl, python3 (for a one-liner snapshot-hash lookup).
#   Network access to huggingface.co during this script only; the binary
#   itself never touches HF once the cache is populated.

set -euo pipefail

OUT_DIR="${1:-./models}"
mkdir -p "$OUT_DIR"

HF_ENDPOINT="${HF_ENDPOINT:-https://huggingface.co}"

# (repo, file, file, ...) — the fetch order doesn't matter; cache layout is stable.
declare -a REPOS=(
    "onnx-community/gliner_small-v2.1:onnx/model_quantized.onnx:tokenizer.json:config.json:gliner_config.json:spm.model"
    "mixedbread-ai/mxbai-edge-colbert-v0-17m:onnx/model.onnx:tokenizer.json:config.json"
    "Xenova/bge-small-en-v1.5:onnx/model.onnx:tokenizer.json:config.json:special_tokens_map.json:tokenizer_config.json"
)

# Resolve `main` → commit sha once per repo so blob paths stay content-addressed.
resolve_sha () {
    local repo="$1"
    curl -fsSL "$HF_ENDPOINT/api/models/$repo/revision/main" \
        | python3 -c "import sys, json; print(json.load(sys.stdin)['sha'])"
}

fetch_file () {
    local repo="$1" rel="$2" sha="$3"
    local root="$OUT_DIR/models--$(echo "$repo" | sed 's@/@--@g')"
    local snap="$root/snapshots/$sha"
    local blob_dir="$root/blobs"
    mkdir -p "$snap/$(dirname "$rel")" "$blob_dir" "$root/refs"

    local url="$HF_ENDPOINT/$repo/resolve/$sha/$rel"
    local target="$snap/$rel"

    if [[ -f "$target" ]]; then
        echo "  [skip] $repo/$rel already present"
        return 0
    fi

    echo "  [fetch] $repo/$rel"
    # Content-address by sha256 of the file once downloaded; hf-hub tolerates
    # either layout (symlink-to-blob or direct file) so a plain file is fine.
    curl -fsSL "$url" -o "$target.part"
    mv "$target.part" "$target"

    # Record main → sha for hf-hub resolution without re-hitting the network.
    echo -n "$sha" > "$root/refs/main"
}

echo "[fetch-models] output dir: $OUT_DIR"

for entry in "${REPOS[@]}"; do
    IFS=':' read -r -a parts <<< "$entry"
    repo="${parts[0]}"
    echo "[fetch-models] resolving $repo"
    sha=$(resolve_sha "$repo")
    echo "  sha: $sha"
    for (( i=1; i<${#parts[@]}; i++ )); do
        fetch_file "$repo" "${parts[$i]}" "$sha"
    done
done

echo "[fetch-models] done. Set TM_MODELS_DIR=$(cd "$OUT_DIR" && pwd) to use these weights."

# ---------------------------------------------------------------------------
# Qwen2.5-0.5B-Instruct Q4_K_M — Tier-1 candle synthesis backend (~400 MB)
# ---------------------------------------------------------------------------
#
# This GGUF is used by the `candle-llm` feature in `tm-answer` for on-device
# synthesis without cmake. Download separately from the ONNX models above
# because it goes to ~/.tracemind/models/ (not the HF cache layout).

fetch_qwen_gguf () {
    local dest="$HOME/.tracemind/models/qwen2.5-0.5b-instruct-q4_k_m.gguf"
    if [[ -f "$dest" ]]; then
        echo "[fetch-models] Qwen2.5-0.5B GGUF already present at $dest"
        return 0
    fi
    mkdir -p "$(dirname "$dest")"
    echo "[fetch-models] Downloading Qwen2.5-0.5B-Instruct Q4_K_M (~400 MB)..."
    if command -v huggingface-cli &>/dev/null; then
        huggingface-cli download \
            Qwen/Qwen2.5-0.5B-Instruct-GGUF \
            qwen2.5-0.5b-instruct-q4_k_m.gguf \
            --local-dir "$(dirname "$dest")"
    else
        curl -fL \
            "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf" \
            -o "$dest"
    fi
    echo "[fetch-models] Qwen2.5-0.5B GGUF ready at $dest"
}

fetch_qwen_gguf
