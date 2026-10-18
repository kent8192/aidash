# syntax=docker/dockerfile:1
# Base images are pinned by digest so a rebuild starts from the same layers (#151).
# Refresh them deliberately, together with broker-image.json.
FROM rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
COPY enterprise ./enterprise
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/enterprise/target \
    cargo build --locked --release --manifest-path enterprise/Cargo.toml -p aidash-broker && \
    cp enterprise/target/release/aidash-broker /aidash-broker
FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /aidash-broker /usr/local/bin/aidash-broker
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["aidash-broker"]
