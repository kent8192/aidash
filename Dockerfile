# syntax=docker/dockerfile:1
FROM rust:1.96-bookworm AS rust
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY migration ./migration
COPY src ./src
ARG CARGO_PROFILE=release
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --profile "$CARGO_PROFILE" --bin aidash --bin local-dev-db && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then \
      cp target/debug/aidash /out/aidash && cp target/debug/local-dev-db /out/local-dev-db; \
    else \
      cp "target/$CARGO_PROFILE/aidash" /out/aidash && cp "target/$CARGO_PROFILE/local-dev-db" /out/local-dev-db; \
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
COPY --from=rust /out/local-dev-db /usr/local/bin/local-dev-db
USER 10001:10001
ENV AIDASH_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["aidash"]
CMD ["serve"]

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=rust /out/aidash /usr/local/bin/aidash
COPY --from=web /build/web/dist /app/web
USER 10001:10001
ENV AIDASH_LISTEN=0.0.0.0:8080 AIDASH_WEB_DIR=/app/web
EXPOSE 8080 8081
STOPSIGNAL SIGTERM
ENTRYPOINT ["aidash"]
CMD ["serve"]
