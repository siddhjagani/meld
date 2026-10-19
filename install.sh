#!/usr/bin/env bash
# Meld installer:  curl -fsSL https://raw.githubusercontent.com/siddhjagani/meld/main/install.sh | bash
# Downloads the latest release binary for your Mac. Never builds from source.
set -euo pipefail

REPO="${MELD_REPO:-siddhjagani/meld}"          # override with MELD_REPO=you/meld
INSTALL_DIR="${MELD_INSTALL_DIR:-/usr/local/bin}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

say()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || fail "this installer is for macOS — on Windows run: irm https://raw.githubusercontent.com/siddhjagani/meld/main/install.ps1 | iex"

case "$(uname -m)" in
  arm64)  TARGET="aarch64-apple-darwin" ;;
  x86_64) TARGET="x86_64-apple-darwin" ;;
  *)      fail "unsupported architecture: $(uname -m)" ;;
esac

# Direct "latest" download link: no GitHub API call, so no API rate limit
# (60/hour per IP, easily used up on shared networks).
ASSET_URL="https://github.com/$REPO/releases/latest/download/meld-$TARGET.tar.gz"

say "downloading meld ($TARGET)"
curl -fsSL "$ASSET_URL" -o "$TMP/meld.tar.gz" \
  || fail "could not download $ASSET_URL — check your connection, or see https://github.com/$REPO/releases"
tar -xzf "$TMP/meld.tar.gz" -C "$TMP"

say "installing to $INSTALL_DIR/meld"
if [ -w "$INSTALL_DIR" ]; then
  install -m 755 "$TMP/meld" "$INSTALL_DIR/meld"
else
  sudo install -m 755 "$TMP/meld" "$INSTALL_DIR/meld"
fi

say "installed: $("$INSTALL_DIR/meld" --version)"
say "next steps:  meld doctor  →  meld sync --dry-run  →  meld sync"
