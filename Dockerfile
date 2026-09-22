# This Dockerfile must be built with the repository root as the Docker context.
# For platforms that assume the context equals the Dockerfile directory, use the
# repository-root Dockerfile instead.

# Build stage
FROM rust:1.88.0-slim-bookworm AS builder

# Install build dependencies. git is needed because txio-cli (built below) is
# a separate repository that depends on this one's `api` crate via a relative
# path, so its source has to be fetched alongside this repo's own.
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*

# txio-backend and txio-cli are laid out as sibling directories, matching
# txio-cli's `path = "../txio-backend/api"` dependency on this repo's api crate.
WORKDIR /workspace/txio-backend
COPY Cargo.toml Cargo.lock ./
COPY api ./api

# Allow pinning a specific txio-cli ref for reproducible builds; defaults to
# its default branch.
ARG TXIO_CLI_REF=main
RUN git clone --depth 1 --branch "${TXIO_CLI_REF}" https://github.com/Txio-labs/txio-cli.git /workspace/txio-cli

# txio-cli's Cargo.toml was written for a monorepo layout (a shared root
# workspace with `backend/` and `cli/` as siblings) that predates the repos
# being split apart. It still uses `*.workspace = true` inheritance and a
# `path = "../backend/api"` dependency, neither of which resolve once
# txio-cli and txio-backend are separate repositories/checkouts. Patch the
# clone's manifest in place (not the real repo) to use explicit values and
# the actual sibling directory name used in this build.
RUN sed -i \
    -e 's/^version\.workspace = true/version = "0.1.0"/' \
    -e 's/^edition\.workspace = true/edition = "2021"/' \
    -e 's/^authors\.workspace = true/authors = ["Victor Oladimeji"]/' \
    -e 's/^license\.workspace = true/license = "MIT"/' \
    -e 's/^description\.workspace = true/description = "One terminal. Every chain."/' \
    -e 's#^repository\.workspace = true#repository = "https://github.com/Txio-labs/txio-cli"#' \
    -e 's#^homepage\.workspace = true#homepage = "https://github.com/Txio-labs/txio-cli"#' \
    -e 's#path = "\.\./backend/api"#path = "../txio-backend/api"#' \
    /workspace/txio-cli/Cargo.toml

# The remaining `{ workspace = true }` dependency entries need txio-backend's
# actual pinned versions/features substituted in, since txio-cli has no
# workspace of its own to inherit them from.
RUN sed -i \
    -e 's/^clap = { workspace = true }/clap = { version = "4.6", features = ["derive"] }/' \
    -e 's/^tokio = { workspace = true }/tokio = { version = "1", features = ["full"] }/' \
    -e 's/^serde = { workspace = true }/serde = { version = "1", features = ["derive"] }/' \
    -e 's/^serde_json = { workspace = true }/serde_json = "1"/' \
    -e 's/^reqwest = { workspace = true }/reqwest = { version = "0.11", features = ["json"] }/' \
    -e 's/^anyhow = { workspace = true }/anyhow = "1"/' \
    -e 's/^async-trait = { workspace = true }/async-trait = "0.1"/' \
    -e 's/^regex = { workspace = true }/regex = "1"/' \
    -e 's/^dotenvy = { workspace = true }/dotenvy = "0.15"/' \
    -e 's/^strsim = { workspace = true }/strsim = "0.11"/' \
    -e 's/^clap_complete = { workspace = true }/clap_complete = "4.6"/' \
    -e 's/^colored = { workspace = true }/colored = "2.2"/' \
    -e 's/^indicatif = { workspace = true }/indicatif = "0.17"/' \
    -e 's/^dirs-next = { workspace = true }/dirs-next = "2.0"/' \
    -e 's/^dialoguer = { workspace = true }/dialoguer = "0.11"/' \
    -e 's/^bs58 = { workspace = true }/bs58 = "0.5"/' \
    /workspace/txio-cli/Cargo.toml

# txio-cli's own Cargo.lock was generated against the pre-split workspace
# layout and is now stale (different member set), so it has to be
# regenerated rather than reused.
RUN rm -f /workspace/txio-cli/Cargo.lock

WORKDIR /workspace/txio-backend
RUN cargo build --release --package txio-api

WORKDIR /workspace/txio-cli
RUN cargo build --release --package txio

# Runtime stage
# Use a minimal base image for runtime to reduce attack surface
FROM debian:bookworm-slim

# Add a non-root system user so the process never runs as root.
RUN useradd --system --uid 10001 txio

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 curl && rm -rf /var/lib/apt/lists/*

# Copy backend binary
COPY --from=builder /workspace/txio-backend/target/release/txio-api /app/api

# Copy CLI binary to system PATH so TerminalService can find it
COPY --from=builder /workspace/txio-cli/target/release/txio /usr/local/bin/txio

# Create a dedicated non-root user and grant it access to runtime paths.
RUN install -d -o 10001 -g 10001 /app/temp

# Transfer ownership to the non-root user before switching to it.
RUN chown -R txio:txio /app
USER 10001:10001

# Set environment variables
ENV RUST_LOG=info
ENV MONGO_URI=mongodb://mongodb:27017/txio

EXPOSE 8000

# The entrypoint is the backend API
CMD ["./api"]
