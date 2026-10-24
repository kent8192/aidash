# syntax=docker/dockerfile:1
# Build only Marketplace from the repository root, without the Cloud runtime.
FROM rust:1.96-bookworm AS build
ARG CARGO_BUILD_JOBS=2
ARG CARGO_PROFILE_DEV_DEBUG=0
ARG CARGO_INCREMENTAL=0
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
COPY enterprise ./enterprise
ARG CARGO_PROFILE=release
RUN --mount=type=cache,id=aidash-standalone-native-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=aidash-standalone-native-target,target=/build/enterprise/target,sharing=locked \
    cargo build --locked --manifest-path enterprise/Cargo.toml --profile "$CARGO_PROFILE" \
      -p aidash-marketplace-server --bin aidash-marketplace-server && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash-marketplace-server" /out/

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
COPY --from=build /out/aidash-marketplace-server /usr/local/bin/aidash-marketplace-server
COPY --chmod=755 enterprise/marketplace/backend/entrypoint.sh /usr/local/bin/marketplace-entrypoint
USER 10001:10001
ENV AIDASH_MARKETPLACE_LISTEN=0.0.0.0:8082
EXPOSE 8082
STOPSIGNAL SIGTERM
ENTRYPOINT ["marketplace-entrypoint"]
