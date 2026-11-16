# txio-compiler: builds Contract Builder sources for Sui, Aptos, Stellar,
# Solana and Starknet. Build from the txio-backend directory:
#
#   docker build -f compiler/Dockerfile -t txio-compiler .
#   docker run -d --restart unless-stopped -p 8090:8090 \
#     --memory 4g --memory-swap 4g --cpus 2 --pids-limit 512 \
#     -v txio-compiler-cache:/home/compiler/.cache/txio-compiler \
#     -e COMPILER_ALLOWED_ORIGINS=https://your.app txio-compiler
#
# Memory: one build at a time with two rustc jobs (the defaults below) peaks
# around 2-3 GB, so a 4 GB limit leaves room. Raise COMPILER_MAX_CONCURRENT
# only with more memory. Keep the cache on a disk volume, never a tmpfs: a
# tmpfs is RAM. After every compile the service kills the build's leftover
# processes, deletes its own artifacts, and prunes the cache back under
# COMPILER_CACHE_MAX_MB. The root filesystem must stay writable: cargo
# downloads crates into CARGO_HOME on first use.
#
# This container compiles code that users submit. Give it no secrets, no
# credentials and no access to other services; it needs outbound HTTPS only
# to fetch crate/package dependencies on first use.

FROM rust:1.98.1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY api ./api
COPY compiler ./compiler
RUN cargo build --release --package txio-compiler

FROM rust:1.98.1-slim-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates git bzip2 unzip python3 libssl-dev pkg-config libudev-dev \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 compiler
USER compiler
WORKDIR /home/compiler
ENV PATH=/home/compiler/.local/bin:/home/compiler/.cargo/bin:/home/compiler/.local/share/solana/install/active_release/bin:$PATH

# Pinned toolchain versions; bump deliberately. Stellar, Agave and Scarb
# match the versions this service was tested with. SUI_VERSION and
# APTOS_CLI_VERSION must be set to a current release at build time
# (--build-arg); they were not tested here.
ARG STELLAR_CLI_VERSION=28.1.0
ARG AGAVE_VERSION=v4.2.2
ARG SCARB_VERSION=2.21.0
ARG SUI_VERSION
ARG APTOS_CLI_VERSION

RUN rustup target add wasm32v1-none \
    && cargo install --locked stellar-cli --version ${STELLAR_CLI_VERSION} --no-default-features
RUN sh -c "$(curl -sSfL https://release.anza.xyz/${AGAVE_VERSION}/install)" \
    && cargo-build-sbf --install-only
RUN curl --proto '=https' --tlsv1.2 -sSf https://docs.swmansion.com/scarb/install.sh | sh -s -- -v ${SCARB_VERSION}
RUN test -n "${SUI_VERSION}" -a -n "${APTOS_CLI_VERSION}" || (echo "Set --build-arg SUI_VERSION=mainnet-vX.Y.Z and APTOS_CLI_VERSION=X.Y.Z" >&2; exit 1)
RUN mkdir -p .local/bin \
    && curl -sSfL "https://github.com/MystenLabs/sui/releases/download/${SUI_VERSION}/sui-${SUI_VERSION}-ubuntu-x86_64.tgz" | tar -xz -C .local/bin ./sui \
    && curl -sSfL "https://github.com/aptos-labs/aptos-core/releases/download/aptos-cli-v${APTOS_CLI_VERSION}/aptos-cli-${APTOS_CLI_VERSION}-Ubuntu-22.04-x86_64.zip" -o aptos.zip \
    && unzip -o aptos.zip -d .local/bin && rm aptos.zip

COPY --from=build /src/target/release/txio-compiler /usr/local/bin/txio-compiler
RUN mkdir -p /home/compiler/.cache/txio-compiler
ENV COMPILER_CACHE_DIR=/home/compiler/.cache/txio-compiler \
    COMPILER_MAX_CONCURRENT=1 \
    COMPILER_BUILD_JOBS=2 \
    COMPILER_CACHE_MAX_MB=3072
EXPOSE 8090
CMD ["txio-compiler"]
