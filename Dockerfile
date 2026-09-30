# This Dockerfile must be built with the repository root as the Docker context.
# For platforms that assume the context equals the Dockerfile directory, use the
# repository-root Dockerfile instead.

# Build stage
FROM rust:1.98.1-slim-bookworm AS builder

# Install build dependencies. git is needed because txio-cli (built below) is
# a separate repository that depends on this one's `api` crate via a relative
# path, so its source has to be fetched alongside this repo's own.
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*

# txio-backend and txio-cli are laid out as sibling directories, matching
# txio-cli's `path = "../txio-backend/api"` dependency on this repo's api crate.
WORKDIR /workspace/txio-backend
COPY Cargo.toml Cargo.lock ./
COPY api ./api

# TXIO_CLI_REF is a commit SHA (or a tag). Building from a moving branch makes
# the image depend on whatever that branch holds today, so the default is
# deliberately not a branch: pass --build-arg TXIO_CLI_REF=<sha>. `git fetch`
# accepts a SHA where `git clone --branch` does not.
ARG TXIO_CLI_REF
RUN test -n "${TXIO_CLI_REF}" || (echo "Set --build-arg TXIO_CLI_REF=<commit sha or tag>" >&2; exit 1)
RUN git init /workspace/txio-cli \
    && git -C /workspace/txio-cli fetch --depth 1 https://github.com/Txio-labs/txio-cli.git "${TXIO_CLI_REF}" \
    && git -C /workspace/txio-cli checkout --detach FETCH_HEAD

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
