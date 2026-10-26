# Build and install on Linux (Ubuntu)

How to turn a checkout of this fork into an installable `.deb` or AppImage, and how to rebuild after code changes. For the toolchain itself, see [`dev-setup-linux.md`](dev-setup-linux.md).

## 1. Prerequisites

- Rust stable with `cargo` on `PATH` (`$HOME/.cargo/bin`), plus `rustfmt` and `clippy`
- Node 20+ and `sherlock/desktop/node_modules` (`npm install`)
- Tauri system libraries, `ffmpeg`, `libfuse2t64`
- Native libraries present in `sherlock/desktop/src-tauri/lib/`:

```bash
cd sherlock/desktop/src-tauri
bash scripts/download-pdfium.sh
bash scripts/download-onnxruntime.sh
```

Both scripts skip work when the files already exist.

## 2. Build

```bash
cd ~/repos/FrankSherlock/sherlock/desktop
PATH="$HOME/.cargo/bin:$PATH" npm run tauri:build -- \
  --bundles deb,appimage \
  --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Why the two flags:

- `--bundles deb,appimage` — [`tauri.conf.json`](../sherlock/desktop/src-tauri/tauri.conf.json) sets `bundle.targets` to `all`, which also builds an RPM. Without `rpmbuild` installed that step fails and aborts the build.
- `--config '{"bundle":{"createUpdaterArtifacts":false}}'` — the config enables `createUpdaterArtifacts`, and signing needs `TAURI_SIGNING_PRIVATE_KEY`, which only exists in upstream CI. The override is merged in memory only, so the repository config is unchanged.

[`scripts/build-local.sh`](../scripts/build-local.sh) wraps all of the above: it installs frontend dependencies, runs `cargo test`, and then builds. The script applies the same `createUpdaterArtifacts` override automatically **unless** `TAURI_SIGNING_PRIVATE_KEY` is exported, so a plain local build never asks for the minisign key while a release build still signs its artifacts.

First release build compiles every dependency and takes a few minutes; later builds are incremental. To build only one format, use `--bundles deb` or `--bundles appimage`.

If `cargo` is not found, `tauri dev` and `tauri build` both fail with `failed to run 'cargo metadata'`. Prefix `PATH` as shown, or use [`scripts/dev-linux.sh`](../scripts/dev-linux.sh) for dev runs.

## 3. Artifacts

```text
sherlock/desktop/src-tauri/target/release/bundle/deb/frank_sherlock_<version>_amd64.deb
sherlock/desktop/src-tauri/target/release/bundle/appimage/frank_sherlock_<version>_amd64.AppImage
```

Reference sizes for 0.8.2: 42 MB deb (118 MB installed) and 97 MB AppImage. Both contain the binary, `libonnxruntime.so`, `libpdfium.so` and `surya_ocr.py`.

Inspect before installing:

```bash
B=sherlock/desktop/src-tauri/target/release/bundle
dpkg-deb -I "$B/deb/frank_sherlock_0.8.2_amd64.deb" | sed -n '1,15p'
dpkg-deb -c "$B/deb/frank_sherlock_0.8.2_amd64.deb" | grep -E "usr/bin|\.desktop|surya_ocr|libpdfium"
```

## 4. Install

### .deb (recommended)

```bash
cd ~/repos/FrankSherlock
sudo apt install ./sherlock/desktop/src-tauri/target/release/bundle/deb/frank_sherlock_0.8.2_amd64.deb
```

`apt` resolves the declared dependencies (`libwebkit2gtk-4.1-0`, `libgtk-3-0`); `dpkg -i` alone would not. Launch from Activities as **Frank Sherlock**, or run `sherlock_app`.

Uninstall, keeping data:

```bash
sudo apt remove frank-sherlock
```

Reinstalling a newer build: run the same `apt install ./file.deb`; apt upgrades in place.

### AppImage

```bash
chmod +x sherlock/desktop/src-tauri/target/release/bundle/appimage/frank_sherlock_0.8.2_amd64.AppImage
./sherlock/desktop/src-tauri/target/release/bundle/appimage/frank_sherlock_0.8.2_amd64.AppImage
```

If FUSE is unavailable, run it with `--appimage-extract-and-run`.

Both formats use `~/.local/share/frank_sherlock`, so the index, thumbnails and caches are shared and survive uninstall.

## 5. Verify a build

```bash
cd sherlock/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

cd ..
npx tsc --noEmit
npm run test
```

Smoke test with a throwaway database so the real index is untouched:

```bash
FRANK_SHERLOCK_DATA_DIR=/tmp/frank-build-test sherlock_app
```

## 6. Fork-specific notes

- **Updater endpoint.** [`tauri.conf.json`](../sherlock/desktop/src-tauri/tauri.conf.json) points the updater at `akitaonrails/FrankSherlock`. On the AppImage, **Check for Updates** would download and replace this build with an upstream release. Either avoid that button or change the endpoint to your fork.
- **Version.** The fork keeps `0.8.2`, the same as upstream. For a distinguishable local build, bump `version` in [`tauri.conf.json`](../sherlock/desktop/src-tauri/tauri.conf.json) and [`package.json`](../sherlock/desktop/package.json). Semver treats `0.8.2-fork.1` as lower than `0.8.2`, so prefer a higher patch such as `0.8.3-fork.1` if you also repoint the updater.
- **Release signing.** Building `.sig` files requires `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; that is why local builds disable updater artifacts. Only GitHub Actions sets them (see [`.github/workflows/release.yml`](../.github/workflows/release.yml)).

## 7. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `failed to run 'cargo metadata'` | `cargo` not on `PATH` | `PATH="$HOME/.cargo/bin:$PATH"` before the command |
| `A public key has been found, but no private key` | `createUpdaterArtifacts` enabled without a signing key | Keep the `--config '{"bundle":{"createUpdaterArtifacts":false}}'` override |
| RPM target fails | `rpmbuild` not installed | Use `--bundles deb,appimage` |
| `dlopen(): error loading libfuse.so.2` during AppImage bundling | Missing FUSE 2 runtime | `sudo apt install libfuse2t64` |
| `symbol lookup error ... /snap/core20/.../libpthread.so.0` when running | Snap-packaged editor leaked GTK/GIO paths into the terminal | Run from a normal terminal, or use [`scripts/dev-linux.sh`](../scripts/dev-linux.sh) for dev runs; installed apps launched from the desktop menu are unaffected |
| Blank WebKit window on Wayland with NVIDIA | WebKit DMA-BUF issue | `WEBKIT_DISABLE_DMABUF_RENDERER=1` |

## 8. Rebuild after pulling upstream changes

```bash
git pull
cd sherlock/desktop
npm install            # only when package.json changed
cd src-tauri
bash scripts/download-pdfium.sh
bash scripts/download-onnxruntime.sh
cd ..
PATH="$HOME/.cargo/bin:$PATH" npm run tauri:build -- \
  --bundles deb,appimage \
  --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Migrations run automatically at startup; an existing `~/.local/share/frank_sherlock/db/index.sqlite` is upgraded in place. Back up that directory before installing a build that changes the schema.
