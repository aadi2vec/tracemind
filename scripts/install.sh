#!/usr/bin/env bash
# TraceMind installer — D-5 from the demo punch list.
#
# Usage:
#   curl -fsSL https://tracemind.dev/install.sh | sh
#   curl -fsSL https://tracemind.dev/install.sh | sh -s -- --prefix ~/.local
#
# Behavior:
#   - Detects platform (macOS arm64/x86_64, Linux arm64/x86_64).
#   - Downloads the latest release tarball from the configured GitHub
#     release URL (overridable via TM_INSTALL_RELEASE).
#   - Verifies SHA256 if a checksums file is present alongside the
#     tarball.
#   - Installs three binaries into $PREFIX/bin: tracemind, tm-mcp,
#     tracemind-capture.
#   - Creates ~/.tracemind/ if missing.
#   - Idempotent — re-running upgrades the binaries in place.
#   - On failure leaves nothing behind.
#
# Privacy stance (matches the README): this script downloads a release
# tarball from GitHub and writes binaries to your machine. It does not
# send anything anywhere. The binaries themselves are local-only.

set -euo pipefail

# --- defaults ---------------------------------------------------------------

REPO="${TM_INSTALL_REPO:-aaditya-dev/tracemind}"
RELEASE_TAG="${TM_INSTALL_RELEASE:-latest}"
PREFIX="${TM_INSTALL_PREFIX:-}"
DATA_DIR="${TM_DATA_DIR:-$HOME/.tracemind}"
SKIP_CHECKSUM="${TM_INSTALL_SKIP_CHECKSUM:-0}"

# parse args
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix)
      PREFIX="$2"; shift 2 ;;
    --release)
      RELEASE_TAG="$2"; shift 2 ;;
    --skip-checksum)
      SKIP_CHECKSUM=1; shift ;;
    --help|-h)
      sed -n '2,32p' "$0"; exit 0 ;;
    *)
      echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# --- prefix selection -------------------------------------------------------
# Default precedence:
#   1) /usr/local/bin if writable (preferred — already on PATH)
#   2) $HOME/.local/bin (created if missing; user may need to add to PATH)

if [ -z "$PREFIX" ]; then
  if [ -w /usr/local/bin ] 2>/dev/null; then
    PREFIX="/usr/local"
  else
    PREFIX="$HOME/.local"
  fi
fi
BIN_DIR="$PREFIX/bin"

# --- platform detection -----------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin) OS_TAG="apple-darwin" ;;
  Linux)  OS_TAG="unknown-linux-gnu" ;;
  *) echo "unsupported OS: $OS" >&2; exit 1 ;;
esac

case "$ARCH" in
  arm64|aarch64) ARCH_TAG="aarch64" ;;
  x86_64|amd64)  ARCH_TAG="x86_64" ;;
  *) echo "unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

TARGET="${ARCH_TAG}-${OS_TAG}"
TARBALL="tracemind-${TARGET}.tar.gz"
CHECKSUMS="tracemind-${TARGET}.sha256"

if [ "$RELEASE_TAG" = "latest" ]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/${RELEASE_TAG}"
fi

# --- helpers ----------------------------------------------------------------

say() { printf "  %s\n" "$*"; }
die() { printf "error: %s\n" "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

need uname
need mkdir
need install
if command -v curl >/dev/null 2>&1; then
  DL="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
  DL="wget -qO-"
else
  die "need curl or wget on PATH"
fi

# Check if writes to $PREFIX need sudo, but only escalate when actually needed.
SUDO=""
if ! mkdir -p "$BIN_DIR" 2>/dev/null; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
    $SUDO mkdir -p "$BIN_DIR" || die "cannot create $BIN_DIR"
  else
    die "cannot create $BIN_DIR (no sudo available)"
  fi
fi

# --- download into a temp dir we can clean up -------------------------------

TMP="$(mktemp -d 2>/dev/null || mktemp -d -t tracemind)"
trap 'rm -rf "$TMP"' EXIT

echo "TraceMind installer"
say "platform: $TARGET"
say "release:  $RELEASE_TAG"
say "prefix:   $PREFIX"
say "data:     $DATA_DIR"
echo

say "fetching $TARBALL ..."
$DL "$BASE_URL/$TARBALL" > "$TMP/$TARBALL" \
  || die "download failed (release may not exist for $TARGET — check $BASE_URL)"

if [ "$SKIP_CHECKSUM" != "1" ]; then
  if $DL "$BASE_URL/$CHECKSUMS" > "$TMP/$CHECKSUMS" 2>/dev/null; then
    say "verifying checksum ..."
    expected="$(awk -v f="$TARBALL" '$2==f {print $1}' "$TMP/$CHECKSUMS")"
    if [ -z "$expected" ]; then
      say "  warning: no checksum entry for $TARBALL — skipping verification"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "$TMP/$TARBALL" | awk '{print $1}')"
      [ "$expected" = "$actual" ] || die "checksum mismatch (expected $expected, got $actual)"
    elif command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$TMP/$TARBALL" | awk '{print $1}')"
      [ "$expected" = "$actual" ] || die "checksum mismatch (expected $expected, got $actual)"
    else
      say "  warning: no shasum/sha256sum on PATH — skipping verification"
    fi
  fi
fi

say "extracting ..."
( cd "$TMP" && tar -xzf "$TARBALL" )

# Tarball layout (release-side responsibility):
#   tracemind-${TARGET}/tracemind
#   tracemind-${TARGET}/tm-mcp
#   tracemind-${TARGET}/tracemind-capture
#   tracemind-${TARGET}/LICENSE
#   tracemind-${TARGET}/README.md
EXTRACT_DIR="$TMP/tracemind-${TARGET}"
[ -d "$EXTRACT_DIR" ] || die "tarball missing expected directory tracemind-${TARGET}/"

for bin in tracemind tm-mcp tracemind-capture; do
  src="$EXTRACT_DIR/$bin"
  [ -f "$src" ] || die "tarball missing binary: $bin"
  say "installing $bin -> $BIN_DIR/$bin"
  $SUDO install -m 0755 "$src" "$BIN_DIR/$bin"
done

# Data dir — owned by the running user even when binaries went to /usr/local.
mkdir -p "$DATA_DIR"
mkdir -p "$DATA_DIR/models"

echo
echo "TraceMind installed."
echo "  binaries:  $BIN_DIR/{tracemind, tm-mcp, tracemind-capture}"
echo "  data dir:  $DATA_DIR"
echo

# Helpful PATH hint when we wrote to ~/.local/bin and it's not on PATH.
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo "Note: $BIN_DIR is not on your PATH."
    echo "      Add the following to your shell rc:"
    echo "        export PATH=\"$BIN_DIR:\$PATH\""
    echo
    ;;
esac

echo "Try it:"
echo "  tracemind status"
echo "  tracemind demo restore --force   # deterministic demo fixture"
echo "  tracemind brief                  # the daily brief"
