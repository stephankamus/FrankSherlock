# CLAUDE.md

## Idioma / Language

**As respostas dos agentes (chat, planos, TODOs e artefatos de planejamento) devem ser
escritas em português do Brasil (pt-BR).** Código, identificadores, nomes de arquivos,
mensagens de erro em `AppError`, logs e comentários de código seguem em inglês, como o
resto do repositório.

## Project Overview

Frank Sherlock is a local-only, AI-powered image cataloging and search system. The main application lives in `sherlock/` — a Tauri v2 desktop app with a Rust backend and React frontend. Everything else in this repo is research and prototyping that informed the final design.

## Repository Layout

```
sherlock/               <- THE MAIN APP (Tauri v2 desktop)
  core/                 <- Rust read core shared with the web server
                           (error, query parser, row models, search queries)
  shared/               <- npm workspace package shared by both frontends:
                           read types, pure utils, `LibraryClient`, `useLibrary()`
  desktop/
    src-tauri/          <- Rust backend (scan, classify, thumbnail, publish, config)
    src/                <- React frontend (VSCode-inspired UI)
  web/                  <- Docker-served read-only client (see plans/web-client.md)
_classification/        <- PoC: Python classification pipeline (informed classify.rs)
_research_ab_test/      <- A/B benchmark research (scripts, docs, test files)
  scripts/              <- Benchmark scripts for model selection
  docs/                 <- Research notes: IDEA.md, IDEA2.md, RESULTS.md
  lib/                  <- Shared Python helpers
  test_files/           <- Test corpus (gitignored, not shipped)
  results/              <- Generated benchmark outputs (gitignored)
scripts/                <- Build scripts (build-local.sh)
docs/                   <- App assets (app_icon.png)
.github/workflows/      <- CI/CD (release.yml, ci.yml)
```

## Tech Stack (Main App)

- **Desktop**: Tauri v2.10+ (Rust + WebView)
- **Backend**: Rust (rusqlite, ureq, image, base64, regex, sha2, walkdir)
- **Frontend**: React + Vite + TypeScript
- **AI**: Ollama (qwen2.5vl:7b) for vision classification, Surya OCR in isolated Python venv
- **Database**: SQLite + FTS5 in `~/.local/share/frank_sherlock/db/`
- **Target OS**: Linux (AppImage), macOS (DMG), Windows (MSI)

## Build & Test

The frontend is an npm workspace rooted at the repository (`sherlock/shared` +
`sherlock/desktop`), with one lockfile at the root:

```bash
npm install                              # every workspace, from the repository root
npm run test                             # shared + desktop frontend tests
npm run test --workspace @frank-sherlock/shared
npm run test --workspace sherlock-desktop
npm run build --workspace sherlock-desktop
cd sherlock/desktop && npm run tauri:dev   # launch dev mode
cd sherlock/desktop && cargo test          # rust backend tests (src-tauri/)
cd sherlock/desktop && npm run tauri:build # AppImage/DMG/MSI
```

The shared core is a standalone crate, not a workspace member, and has its own tests:

```bash
cd sherlock/core && cargo test # 35 unit tests (query parser)
```

If Wayland/NVIDIA causes WebKit issues:
```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 GDK_BACKEND=wayland,x11 npm run tauri:dev
```

## Key Rust Modules

| Module | Purpose |
|--------|---------|
| `classify.rs` | Ollama vision LLM pipeline: primary classification (3-attempt + regex salvage), anime enrichment, Surya OCR + LLM fallback, document/receipt extraction |
| `face.rs` | Native ONNX face detection (SCRFD) and recognition (ArcFace), 512-dim embeddings, cosine clustering, person management |
| `thumbnail.rs` | 300px JPEG thumbnails, Lanczos3, skip-if-exists, GIF first-frame |
| `scan.rs` | Two-phase incremental scan: metadata-only discovery (zero reads for unchanged), classification + thumbnail processing, move detection, cache cleanup, cooperative cancellation |
| `core/src/db.rs` | SQLite + FTS5, scan checkpointing, upsert/move/delete, search — shared with the web server |
| `config.rs` | AppPaths resolution, directory creation |
| `lib.rs` | Tauri commands, scan worker spawning, setup/download flow, auto-cleanup, scan cancellation |
| `query_parser.rs` | Natural language query parsing (media type, dates, confidence) |
| `runtime.rs` | Ollama/nvidia-smi status gathering |
| `platform/` | OS abstraction: clipboard, GPU detection, Python venv paths, executable lookup |

## Architecture Principles

- **Read-only**: Never writes to scanned directories. All data goes to `~/.local/share/frank_sherlock/`.
  **Only exception, authorized by the user in the modal**: the opt-in HEIC/NEF conversion writes
  JPEG copies into `FrankSherlock-JPG/` inside the directory of the original. No other feature
  writes to a root, and there is deliberately no fallback to the app cache, so the library is
  not spread across two places. See [`plans/heic-nef-conversion.md`](plans/heic-nef-conversion.md).
- **Incremental**: Unchanged files (same mtime + size) skip fingerprinting and classification entirely — only a scan marker update.
- **Move-aware**: Renamed/moved files detected by fingerprint, preserving all classification data.
- **Resilient**: Scan jobs checkpoint after each file. Interrupted scans resume from last cursor.
- **Local-only**: No cloud APIs. Ollama runs locally, Surya OCR runs in an isolated venv.
- **Multi-OS**: All features must work on Linux, macOS, and Windows. OS-specific code lives exclusively in `src-tauri/src/platform/`. Never use OS-specific paths, commands, or assumptions outside this module. CI tests all three platforms on every push.

## Coding Conventions

- Rust: use `thiserror` for error types via `AppError` enum. Propagate with `?`, never `.unwrap()` in production code.
- All new Rust modules must have `#[cfg(test)] mod tests` with unit tests.
- Frontend: TypeScript strict mode, camelCase for TS types matching Rust's `#[serde(rename_all = "camelCase")]`.
- JSON parsing from LLMs uses a 3-tier fallback: direct parse -> brace-balanced extraction -> regex field salvage.
- Thumbnails and classification caches mirror the source `rel_path` structure under their respective dirs.
- Read-side types and pure helpers (`formatBytes`, `fileName`, `basename`, `errorMessage`, media
  extension checks) live in `sherlock/shared`; `sherlock/desktop/src/types.ts` and `src/utils.ts`
  only add desktop-specific shapes and re-export the rest. Modal CSS shares base styles via `shared-modal.css`.
- A component that must work in both apps talks to `useLibrary()` (`LibraryClient`), never to
  `invoke` or `convertFileSrc` directly. The desktop client is
  [`tauriLibraryClient.ts`](sherlock/desktop/src/tauriLibraryClient.ts:1); the web client is
  `createHttpLibraryClient` in `sherlock/shared`. Every visual component that both apps use
  lives in `sherlock/shared/src/components`, each with its CSS next to it: `ImageTile`,
  `ImageGrid`, `Toolbar`, `ResultsMeta`, `ModalOverlay`, `HelpModal`, `ToastContainer`,
  `AlbumCard`, `SmartFolderCard`, `DirectoryTree`, `DuplicatesView`, `FacesView`,
  `PreviewModal`, `RootCard` and `Sidebar`. `PreviewModal` takes a `renderPdf` slot (desktop
  passes `PdfViewer`, web gets an `<iframe>`), and `FacesView` takes an optional `actions`
  object that only the writer passes.
- Write actions inside shared views are gated by `capabilities.write` / `scan` / `pdfUnlock`;
  the server is the second barrier (it also refuses hidden rows and unknown root mounts).
- **App.tsx is an orchestrator** — it composes hooks and renders layout. Feature-specific state and handlers must live in dedicated hooks under `src/hooks/`. New tool modes (like DuplicatesView, FacesView, PdfPasswordsView) should be self-contained components that manage their own data loading. App.tsx only holds: layout state (sidebar, modals, preview), search/filter state, and thin mode-switching coordination.

## Important Paths

- Database: `~/.local/share/frank_sherlock/db/index.sqlite`
- Thumbnails: `~/.local/share/frank_sherlock/cache/thumbnails/`
- Classification cache: `~/.local/share/frank_sherlock/cache/classifications/`
- Surya venv: `~/.local/share/frank_sherlock/surya_venv/`
- Surya OCR script: `sherlock/desktop/src-tauri/scripts/surya_ocr.py`

## Testing

```bash
# Rust (from sherlock/desktop/src-tauri/)
cargo test                     # desktop backend

# Frontend (from the repository root)
npm run test --workspace sherlock-desktop          # 427 tests
npm run test --workspace @frank-sherlock/shared    # shared package
```

Tests cover: JSON parsing fallbacks, thumbnail generation, incremental scan discovery, DB operations (upsert, touch, delete, FTS), query parsing, scan job persistence, platform abstraction, HTTP client contracts, and all UI components. Desktop test fixtures live in `sherlock/desktop/src/__tests__/fixtures.ts`.

## Database Migration Rules

Schema is managed by `rusqlite_migration` in `db.rs`. Migrations are tracked via `PRAGMA user_version`.

- Never edit or reorder existing migrations — they are identified by position.
- Never modify existing `CREATE TABLE` column definitions in a shipped migration.
- To add a column: append a new `M::up("ALTER TABLE ... ADD COLUMN ... DEFAULT ...")`.
- Every migration must be safe to run on an existing database (idempotent where possible).
- Test with both fresh databases and existing databases.
- Never delete data or drop columns without explicit user consent.

## Release Workflow

- When tagging or retagging a version, **always update the release notes** in `releases/vX.Y.Z.md` to reflect all changes included in that tag.
- Release notes are loaded by the release workflow and included in GitHub Releases automatically.
- The AUR package (`frank-sherlock-bin`) is auto-published via `.github/workflows/aur-publish.yml` when a release is published.
- **Three builds must succeed** on every release: Linux (AppImage), macOS (DMG), Windows (MSI). CI runs on all three platforms — never merge or tag if any platform is broken.
- The AUR PKGBUILD downloads the AppImage and extracts desktop file + icons. If the Tauri `productName`, icon names, or desktop file contents change, the PKGBUILD template in `aur-publish.yml` must be updated to match.
- AUR repo: `ssh://aur@aur.archlinux.org/frank-sherlock-bin.git` (separate from this repo, auto-managed by CI).

## What NOT to Change

- Never modify files in scanned target directories.
- Never remove the FTS5 virtual table without migration logic.
- Keep `csp: null` in `tauri.conf.json` — needed for `convertFileSrc` asset protocol.
- The Surya OCR script must stay compatible with being run from an isolated venv (no system Python imports).
