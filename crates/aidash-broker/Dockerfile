# syntax=docker/dockerfile:1
FROM rust:1.96-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release -p aidash-broker && cp target/release/aidash-broker /aidash-broker
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /aidash-broker /usr/local/bin/aidash-broker
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["aidash-broker"]
