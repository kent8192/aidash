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
      -p aidash-auth-server --bin aidash-auth-server && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash-auth-server" /out/

FROM source AS wasm
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-pack --version 0.15.0 --locked
WORKDIR /build/enterprise/auth
RUN --mount=type=cache,id=aidash-standalone-pages-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-target,target=/build/enterprise/target,sharing=locked \
    bash scripts/wasm-build-release.sh

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
COPY --from=native /out/aidash-auth-server /usr/local/bin/aidash-auth-server
COPY --from=wasm /build/enterprise/auth/dist /app/pages
ENV AIDASH_AUTH_ASSETS_DIR=/app/pages
WORKDIR /app
USER 10001:10001
STOPSIGNAL SIGTERM
EXPOSE 8081 9091
ENTRYPOINT ["aidash-auth-server"]
CMD ["serve"]
