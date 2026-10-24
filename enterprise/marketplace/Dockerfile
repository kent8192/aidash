# syntax=docker/dockerfile:1
# Build from the repository root. One Pages project, one product runtime image.
FROM rust:1.96-bookworm AS source
ARG CARGO_BUILD_JOBS=2
ARG CARGO_PROFILE_DEV_DEBUG=0
ARG CARGO_INCREMENTAL=0
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler python3 && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
COPY enterprise ./enterprise
COPY scripts/package-standalone-pages.py ./scripts/package-standalone-pages.py

FROM source AS native
ARG CARGO_PROFILE=release
RUN --mount=type=cache,id=aidash-standalone-native-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-target,target=/build/enterprise/target,sharing=locked \
    cargo build --locked --manifest-path enterprise/Cargo.toml --profile "$CARGO_PROFILE" \
      -p aidash-marketplace-server --bin aidash-marketplace-server && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash-marketplace-server" /out/

FROM source AS wasm
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-pack --version 0.15.0 --locked
WORKDIR /build/enterprise/marketplace
RUN --mount=type=cache,id=aidash-standalone-pages-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-target,target=/build/enterprise/target,sharing=locked \
    bash scripts/wasm-build-release.sh

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
COPY --from=native /out/aidash-marketplace-server /usr/local/bin/aidash-marketplace-server
COPY --from=wasm /build/enterprise/marketplace/dist /app/pages
COPY --chmod=755 enterprise/marketplace/entrypoint.sh /usr/local/bin/marketplace-entrypoint
ENV AIDASH_MARKETPLACE_ASSETS_DIR=/app/pages
ENV AIDASH_MARKETPLACE_LISTEN=0.0.0.0:8082
WORKDIR /app
USER 10001:10001
STOPSIGNAL SIGTERM
EXPOSE 8082
ENTRYPOINT ["marketplace-entrypoint"]
