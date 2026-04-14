#!/usr/bin/env bash
# TraceMind — one-time dev setup
set -euo pipefail

echo "=== TraceMind Dev Setup ==="

# 1. Rust toolchain
if ! command -v cargo &>/dev/null; then
  echo "[1/4] Installing Rust toolchain..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
else
  echo "[1/4] Rust toolchain ✓ ($(rustc --version))"
fi

# 2. Ensure cargo is on PATH in shell config
SHELL_RC="$HOME/.zshrc"
[[ "$SHELL" == */bash ]] && SHELL_RC="$HOME/.bashrc"
if ! grep -q '.cargo/bin' "$SHELL_RC" 2>/dev/null; then
  echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> "$SHELL_RC"
  echo "[2/4] Added cargo to $SHELL_RC"
else
  echo "[2/4] Cargo PATH ✓"
fi

# 3. Build
echo "[3/4] Building (release)..."
cargo build --release 2>&1 | tail -3

# 4. Test
echo "[4/4] Running tests..."
cargo test --workspace 2>&1 | grep -E "^test result:" | while read line; do echo "  $line"; done

echo ""
echo "=== Setup complete ==="
echo ""
echo "Commands:"
echo "  cargo build --release          # Build"
echo "  cargo test --workspace         # Test all crates"
echo "  ./target/release/tracemind     # CLI"
echo "  cargo run -p tm-bench          # Run quality benchmark"
echo "  TM_EMBED_MODEL=minilm ./target/release/tracemind ingest \"text\"  # Use different model"
