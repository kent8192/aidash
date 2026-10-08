# syntax=docker/dockerfile:1
FROM rust:1.96-bookworm AS rust
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY server ./server
ARG CARGO_PROFILE=release
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --profile "$CARGO_PROFILE" -p aidash-server --bin aidash --bin manage --bin local-dev-db && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then \
      cp target/debug/aidash target/debug/manage target/debug/local-dev-db /out/; \
    else \
      cp "target/$CARGO_PROFILE/aidash" "target/$CARGO_PROFILE/manage" "target/$CARGO_PROFILE/local-dev-db" /out/; \
    fi && \
    /out/aidash openapi > /out/aidash.json

FROM node:22-bookworm-slim AS web-source
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web ./
COPY --from=rust /out/aidash.json /build/openapi/aidash.json

FROM web-source AS dev-web
RUN npm run generate:api

FROM web-source AS web
RUN npm run generate:api && npx tsc -b && npx vite build

FROM nginxinc/nginx-unprivileged:1.31-alpine3.24 AS frontend
COPY --from=web /build/web/dist /usr/share/nginx/html
COPY deploy/helm/aidash/files/default.conf /etc/nginx/conf.d/default.conf
EXPOSE 8080

# Compose uses this smaller image for the backend development service. The
# frontend runs separately, so editing Rust does not rebuild the web bundle.
FROM debian:bookworm-slim AS dev-backend
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
COPY --from=rust /out/aidash /usr/local/bin/aidash
COPY --from=rust /out/manage /usr/local/bin/manage
COPY --from=rust /out/local-dev-db /usr/local/bin/local-dev-db
COPY server/migrations /app/server/migrations
COPY server/settings/base.example.toml /app/server/settings/base.toml
RUN sed -i '/^\[core\]$/a base_dir = "/app/server"' /app/server/settings/base.toml
RUN mkdir -p /var/lib/aidash/memory-recovery && chown -R 10001:10001 /var/lib/aidash
WORKDIR /app/server
USER 10001:10001
ENV AIDASH_LISTEN=0.0.0.0:8080 AIDASH_BASE_DIR=/app/server REINHARDT_SETTINGS_DIR=/app/server/settings
EXPOSE 8080
ENTRYPOINT ["aidash"]
CMD ["serve"]

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=rust /out/aidash /usr/local/bin/aidash
COPY --from=rust /out/manage /usr/local/bin/manage
COPY server/migrations /app/server/migrations
COPY server/settings/base.example.toml /app/server/settings/base.toml
RUN sed -i '/^\[core\]$/a base_dir = "/app/server"' /app/server/settings/base.toml
COPY --from=web /build/web/dist /app/web
RUN mkdir -p /var/lib/aidash/memory-recovery && chown -R 10001:10001 /var/lib/aidash
WORKDIR /app/server
USER 10001:10001
ENV AIDASH_LISTEN=0.0.0.0:8080 AIDASH_WEB_DIR=/app/web AIDASH_BASE_DIR=/app/server REINHARDT_SETTINGS_DIR=/app/server/settings
EXPOSE 8080 8081
STOPSIGNAL SIGTERM
ENTRYPOINT ["aidash"]
CMD ["serve"]
