# syntax=docker/dockerfile:1
# Build from the repository root; no Cloud binary or Cloud migrations are run.
FROM rust:1.96-bookworm AS build
ARG CARGO_BUILD_JOBS=2
ARG CARGO_PROFILE_DEV_DEBUG=0
ARG CARGO_INCREMENTAL=0
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
COPY enterprise ./enterprise
ARG CARGO_PROFILE=release
# Parallel image builds must not unpack into one mutable Cargo cache.
RUN --mount=type=cache,id=aidash-standalone-native-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-target,target=/build/enterprise/target,sharing=locked \
    cargo build --locked --manifest-path enterprise/Cargo.toml --profile "$CARGO_PROFILE" \
      -p aidash-auth-server --bin aidash-auth-server && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash-auth-server" /out/

FROM rust:1.96-bookworm AS wasm
ARG CARGO_BUILD_JOBS=2
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-pack --version 0.15.0 --locked
WORKDIR /frontend
COPY enterprise/auth/frontend/Cargo.toml enterprise/auth/frontend/Cargo.lock ./
COPY enterprise/auth/frontend/src ./src
RUN --mount=type=cache,id=aidash-standalone-pages-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-pages-target,target=/frontend/target,sharing=locked \
    wasm-pack build . --target web --out-dir pkg --out-name aidash_auth_web -- --locked

FROM node:24.12.0-bookworm-slim AS web
WORKDIR /build
COPY enterprise/auth/frontend/package.json enterprise/auth/frontend/package-lock.json ./
RUN npm ci --ignore-scripts
COPY enterprise/auth/frontend ./
COPY --from=wasm /frontend/pkg ./pkg
RUN npm run build:assets

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /out/aidash-auth-server /usr/local/bin/aidash-auth-server
COPY --from=web /build/dist /app/auth-web
ENV AIDASH_AUTH_ASSETS_DIR=/app/auth-web
WORKDIR /app
USER 10001:10001
STOPSIGNAL SIGTERM
EXPOSE 8081 9091
ENTRYPOINT ["aidash-auth-server"]
CMD ["serve"]
