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
# Usage: bash scripts/dev-linux.sh [--clean] [extra tauri dev args]
#
# --clean drops the cached artifacts of the two local crates before launching.
# Reach for it when the link step fails with rust-lld errors such as
#
#   rust-lld: error: undefined hidden symbol: anon.<hash>.N.llvm.<hash>
#   collect2: error: ld returned 1 exit status
#
# Those come from a stale `libsherlock_core` rlib left behind by a build that
# was killed mid-write, not from the sources: `cargo check` and `cargo test`
# still pass, only linking fails.

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

clean=0
args=()
for arg in "$@"; do
    if [[ "$arg" == "--clean" ]]; then
        clean=1
    else
        args+=("$arg")
    fi
done

# rustup installs here; a freshly installed toolchain is not on PATH yet.
export PATH="$HOME/.cargo/bin:$PATH"

cd "$DESKTOP_DIR"

if [[ "$clean" == 1 ]]; then
    echo "dev-linux: clearing cached sherlock_core/sherlock_app artifacts"
    (cd src-tauri && cargo clean -p sherlock_core -p sherlock_app)
fi

exec npm run tauri:dev -- "${args[@]}"
