# syntax=docker/dockerfile:1
# Build from the repository root; no Cloud binary or Cloud migrations are run.
FROM rust:1.96-bookworm AS build
ARG CARGO_BUILD_JOBS=2
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
COPY enterprise ./enterprise
ARG CARGO_PROFILE=release
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/enterprise/target \
    cargo build --locked --manifest-path enterprise/Cargo.toml --profile "$CARGO_PROFILE" \
      -p aidash-auth-server --bin aidash-auth-server && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash-auth-server" /out/

FROM node:24.12.0-bookworm-slim AS web
WORKDIR /build
COPY enterprise/auth-web/package.json enterprise/auth-web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY enterprise/auth-web ./
RUN npm run build

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
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
