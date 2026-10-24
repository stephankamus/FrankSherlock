# Local development setup on Linux (Ubuntu)

Build and run Frank Sherlock from source. The app is Tauri v2: a Rust backend in `sherlock/desktop/src-tauri` and a React frontend in `sherlock/desktop`.

Minimum Rust version is `1.77.2` (see [`Cargo.toml`](../sherlock/desktop/src-tauri/Cargo.toml)).

## 1. Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup toolchain install stable --component rustfmt clippy
rustc --version
cargo --version
```

`rustup` installs into `~/.cargo/bin`. If a new terminal cannot find `cargo`, add `source "$HOME/.cargo/env"` to your shell profile.

## 2. System libraries

Ubuntu 24.04:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config libssl-dev curl wget file \
  libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev librsvg2-dev patchelf \
  libayatana-appindicator3-dev \
  ffmpeg libfuse2t64 python3-venv
```

Ubuntu 22.04 uses `libappindicator3-dev` and `libfuse2` instead of the `ayatana`/`t64` packages.

These mirror the Linux step in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml). `ffmpeg` is needed for video thumbnails and metadata; `python3-venv` is used by the app to provision the Surya OCR environment.

If `apt-cache policy libssl-dev` prints no candidate, run `sudo apt-get update` first.

## 3. Node.js

Node 20 or newer (CI uses 24):

```bash
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
source "$HOME/.nvm/nvm.sh"
nvm install 24
cd sherlock/desktop
npm install
```

## 4. Native libraries

PDFium (PDF rendering) and ONNX Runtime (face detection) are not vendored. Download them once:

```bash
cd sherlock/desktop/src-tauri
bash scripts/download-pdfium.sh
bash scripts/download-onnxruntime.sh
```

Both place files in `sherlock/desktop/src-tauri/lib/`.

## 5. Ollama

```bash
curl -fsSL https://ollama.com/install.sh | sh
ollama serve &
ollama pull qwen2.5vl:7b
```

The app picks a model from detected hardware. To force one, edit `~/.config/frank_sherlock/settings.toml`:

```toml
model_override = "qwen2.5vl:3b"
```

## 6. Build, test and run

```bash
cd sherlock/desktop/src-tauri && cargo test
cd sherlock/desktop && npm run test
cd sherlock/desktop && npm run tauri:dev
```

If your terminal was opened from a snap-packaged editor (VS Code, for example), use the wrapper instead. It clears the snap GTK/GIO variables that otherwise make the native binary load `core20` libraries:

```bash
bash scripts/dev-linux.sh
```

The wrapper also adds `$HOME/.cargo/bin` to `PATH`, which `tauri dev` needs to find `cargo`.

Formatting and lint gates used by CI:

```bash
cd sherlock/desktop/src-tauri
cargo fmt --check
cargo clippy -- -D warnings
```

Release bundles (AppImage, and the platform equivalents):

```bash
cd sherlock/desktop
npm run tauri:build
```

On Ubuntu, build only the formats you can produce locally and disable updater signing:

```bash
PATH="$HOME/.cargo/bin:$PATH" npm run tauri:build -- \
  --bundles deb,appimage \
  --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Output lands in `sherlock/desktop/src-tauri/target/release/bundle/`. For installation, package contents, verification and troubleshooting, see [`build-and-install-linux.md`](build-and-install-linux.md).

## 7. Testing against a scratch database

Never point a development build at your real index unless you mean to. Use a throwaway data directory:

```bash
FRANK_SHERLOCK_DATA_DIR=/tmp/frank-test npm run tauri:dev
```

The environment variable overrides `~/.local/share/frank_sherlock` for the database, thumbnails and caches (see [`config.rs`](../sherlock/desktop/src-tauri/src/config.rs)).

## Troubleshooting

| Symptom | Fix |
|---|---|
| Blank WebKit window on Wayland with NVIDIA | `WEBKIT_DISABLE_DMABUF_RENDERER=1 GDK_BACKEND=wayland,x11 npm run tauri:dev` |
| `symbol lookup error: /snap/core20/.../libpthread.so.0: undefined symbol: __libc_pthread_init` | Snap-packaged editor leaked GTK paths into the terminal. Run `bash scripts/dev-linux.sh` |
| `failed to run 'cargo metadata'` from `tauri dev` | `cargo` is not on `PATH`; use `bash scripts/dev-linux.sh` or `export PATH="$HOME/.cargo/bin:$PATH"` |
| `webkit2gtk-4.1` not found by `pkg-config` | Install `libwebkit2gtk-4.1-dev` and `libjavascriptcoregtk-4.1-dev` |
| `error: linker cc not found` | Install `build-essential` |
| ONNX or PDFium load error at runtime | Re-run the download scripts in section 4 |
| Ollama unreachable during scans | `ollama serve`, then `ollama list` to confirm the model is present |
