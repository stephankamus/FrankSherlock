#!/usr/bin/env bash
set -euo pipefail

# Build Frank Sherlock AppImage locally on Arch Linux.
# Usage: ./scripts/build-local.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DESKTOP_DIR="$PROJECT_ROOT/sherlock/desktop"

echo "==> Installing frontend dependencies"
cd "$DESKTOP_DIR"
npm install

echo "==> Running Rust tests"
cd "$DESKTOP_DIR/src-tauri"
cargo test

echo "==> Building Tauri app (release)"
cd "$DESKTOP_DIR"
# `bundle.createUpdaterArtifacts` is true in tauri.conf.json, so a plain
# `tauri build` insists on signing the updater artifacts and fails with
# "A public key has been found, but no private key" when the minisign key is
# not exported. Local builds don't ship through the updater, so we turn the
# artifacts off unless the key is actually available (that's the release case).
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "    signing updater artifacts (TAURI_SIGNING_PRIVATE_KEY is set)"
  npm run tauri:build
else
  echo "    no TAURI_SIGNING_PRIVATE_KEY: building without updater artifacts"
  npm run tauri:build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'
fi

echo ""
echo "==> Build complete!"
echo "    AppImage: $(find "$DESKTOP_DIR/src-tauri/target/release/bundle" -name '*.AppImage' 2>/dev/null | head -1)"
