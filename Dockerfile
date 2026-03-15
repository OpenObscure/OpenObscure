# OpenObscure Gateway — Multi-stage Docker build
#
# Two image variants:
#   slim  — regex + keywords + NER, no voice (sherpa-onnx). ~80MB compressed.
#           Built with: --no-default-features --features server
#           FPE key required: OPENOBSCURE_MASTER_KEY env var, /run/secrets, or volume mount.
#           No OS keychain available in containers — see docs/get-started/docker-quick-start.md
#
#   full  — all models baked in (NER, NSFW, face, OCR, response-integrity). ~600MB compressed.
#           Requires git lfs pull before build. Run: make docker-full

# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM ubuntu:24.04 AS builder

RUN apt-get update && apt-get install -y \
    curl pkg-config libssl-dev cmake g++ \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

# Copy workspace files
COPY openobscure-core/Cargo.toml openobscure-core/Cargo.lock ./openobscure-core/
COPY openobscure-core/src ./openobscure-core/src
COPY openobscure-core/examples ./openobscure-core/examples
COPY openobscure-core/benches ./openobscure-core/benches
COPY openobscure-core/config ./openobscure-core/config

# Build slim binary — server only, voice feature dropped (no sherpa-onnx dependency)
WORKDIR /build/openobscure-core
RUN RUSTFLAGS="-C link-arg=-lstdc++" cargo build --release --no-default-features --features server

# ── Stage 2: Runtime — slim (no models) ───────────────────────────────────────
FROM ubuntu:24.04 AS slim

RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN useradd -r -u 10000 -m -d /home/oo oo
USER oo
WORKDIR /home/oo

COPY --from=builder /build/openobscure-core/target/release/openobscure /usr/local/bin/openobscure
COPY --from=builder /build/openobscure-core/config ./config

# Listen on all interfaces by default so the container is reachable from the host.
# Local installs are unaffected — they never use this image.
ENV OPENOBSCURE_LISTEN_ADDR=0.0.0.0

EXPOSE 18790
ENTRYPOINT ["openobscure", "serve"]

# ── Stage 3: Runtime — full (with models) ─────────────────────────────────────
# Requires: git lfs pull  before running docker build --target full
FROM slim AS full

# Models are large (several GB total with Git LFS) — only baked in for the full image.
# Each model sub-directory matches the path expected by openobscure.toml defaults.
COPY --chown=10000:10000 openobscure-core/models ./models
