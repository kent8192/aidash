# syntax=docker/dockerfile:1
# The Enterprise build's server image. Build it from the repository root:
#   docker build -f enterprise/Dockerfile --target runtime .
# It goes only to Aidash's private registry and is never published.
FROM rust:1.96-bookworm AS rust
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
      -p aidash-enterprise-server --bin aidash --bin manage && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then directory=debug; else directory="$CARGO_PROFILE"; fi && \
    cp "enterprise/target/$directory/aidash" "enterprise/target/$directory/manage" /out/ && \
    /out/aidash openapi > /out/aidash.json

FROM node:24.12.0-bookworm-slim AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web ./
COPY enterprise/web /build/enterprise/web
COPY --from=rust /out/aidash.json /build/openapi/aidash.json
RUN npm run generate:api
WORKDIR /build/enterprise/web
RUN npm run build

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=rust /out/aidash /usr/local/bin/aidash
COPY --from=rust /out/manage /usr/local/bin/manage
COPY server/migrations /app/server/migrations
COPY deploy/demo-data/structure.json /app/deploy/demo-data/structure.json
COPY enterprise/server/migrations /app/enterprise/server/migrations
COPY server/settings/base.example.toml /app/server/settings/base.toml
# Aidash's Environments run the Cloud Offering; a Dedicated Deployment
# overrides the setting.
RUN sed -i '/^\[core\]$/a base_dir = "/app/server"' /app/server/settings/base.toml && \
    sed -i 's/^offering = "self-hosted"$/offering = "cloud"/' /app/server/settings/base.toml && \
    grep -qx 'offering = "cloud"' /app/server/settings/base.toml
COPY --from=web /build/enterprise/web/dist /app/web
RUN mkdir -p /var/lib/aidash/memory-recovery && chown -R 10001:10001 /var/lib/aidash
WORKDIR /app/server
USER 10001:10001
ENV AIDASH_LISTEN=0.0.0.0:8080 AIDASH_WEB_DIR=/app/web AIDASH_BASE_DIR=/app/server REINHARDT_SETTINGS_DIR=/app/server/settings
EXPOSE 8080 8081
STOPSIGNAL SIGTERM
ENTRYPOINT ["aidash"]
CMD ["serve"]
