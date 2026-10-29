# syntax=docker/dockerfile:1

# `aligned 0.4.3` (pulled in by image -> rav1e -> av-scenechange) declares
# `edition = "2024"`, which only exists from Rust 1.85 on. On 1.82 the build dies while
# parsing that manifest:
#
#   feature `edition2024` is required
#   The package requires the Cargo feature called `edition2024` ...
#
# This must stay at or above the MSRV of the locked dependencies; CI uses `stable`.
# Declared before the first FROM so the `build` stage below can expand it.
ARG RUST_VERSION=1.99

# ── Frontend bundle ─────────────────────────────────────────────────────
FROM node:24-bookworm-slim AS ui
WORKDIR /src

# Only the package manifests first, so a source change does not re-download the tree.
COPY package.json package-lock.json ./
COPY sherlock/shared/package.json sherlock/shared/
COPY sherlock/web/ui/package.json sherlock/web/ui/
COPY sherlock/desktop/package.json sherlock/desktop/
RUN npm ci --workspace @frank-sherlock/shared --workspace @frank-sherlock/web \
    --no-audit --no-fund

COPY sherlock/shared sherlock/shared
COPY sherlock/web/ui sherlock/web/ui
RUN npm run build --workspace @frank-sherlock/web

# ── Server, with the bundle compiled in ─────────────────────────────────
FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src

COPY sherlock/core sherlock/core
COPY sherlock/web/server sherlock/web/server
# `assets.rs` embeds this folder at compile time.
COPY --from=ui /src/sherlock/web/ui/dist sherlock/web/ui/dist

# `--locked`: the committed Cargo.lock is the contract. Without it cargo re-resolves
# when the lock drifts and silently pulls crates newer than this toolchain supports.
RUN cargo build --release --locked --features embed-ui \
    --manifest-path sherlock/web/server/Cargo.toml

# ── Runtime ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN useradd --system --uid 10001 --home /app sherlock
WORKDIR /app
COPY --from=build /src/sherlock/web/server/target/release/sherlock-web /app/sherlock-web

ENV LIBRARY_DIR=/library \
    MEDIA_MAP=/config/media.map.json \
    CACHE_DIR=/tmp/fs-web-cache \
    PORT=8080 \
    RUST_LOG=info

EXPOSE 8080
USER sherlock

# Answers without opening the listener; `docker inspect` reports it as unhealthy when
# the publish folder or its pointer is missing.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
    CMD ["/app/sherlock-web", "--health"]

ENTRYPOINT ["/app/sherlock-web"]
