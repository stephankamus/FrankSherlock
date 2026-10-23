#!/usr/bin/env bash
# Launch the Tauri dev app with an environment that does not leak snap libraries.
#
# VS Code installed as a snap exports GTK/GIO paths and SNAP_LIBRARY_PATH into
# its integrated terminal. A native Tauri binary started from that shell then
# resolves libpthread from /snap/core20 and dies with:
#
#   symbol lookup error: /snap/core20/current/lib/x86_64-linux-gnu/libpthread.so.0:
#   undefined symbol: __libc_pthread_init, version GLIBC_PRIVATE
#
# Usage: bash scripts/dev-linux.sh [extra tauri dev args]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$SCRIPT_DIR/../sherlock/desktop"

unset GTK_PATH GTK_EXE_PREFIX GTK_IM_MODULE_FILE \
      GDK_PIXBUF_MODULEDIR GDK_PIXBUF_MODULE_FILE \
      GIO_MODULE_DIR GSETTINGS_SCHEMA_DIR LOCPATH \
      SNAP_LIBRARY_PATH

# Only drop the loader overrides when they point into the snap runtime;
# legitimate values (for example NVIDIA's) are kept.
if [[ "${LD_LIBRARY_PATH:-}" == *"/snap/"* ]]; then
    unset LD_LIBRARY_PATH
fi
if [[ "${LD_PRELOAD:-}" == *"/snap/"* ]]; then
    unset LD_PRELOAD
fi

# rustup installs here; a freshly installed toolchain is not on PATH yet.
export PATH="$HOME/.cargo/bin:$PATH"

cd "$DESKTOP_DIR"
exec npm run tauri:dev -- "$@"
