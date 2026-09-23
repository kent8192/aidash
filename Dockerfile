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
    cargo build --locked --profile "$CARGO_PROFILE" --bin aidash && \
    mkdir -p /out && \
    if [ "$CARGO_PROFILE" = dev ]; then cp target/debug/aidash /out/aidash; \
    else cp "target/$CARGO_PROFILE/aidash" /out/aidash; fi && \
    /out/aidash openapi > /out/aidash.json

FROM node:22-bookworm-slim AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web ./
COPY --from=rust /out/aidash.json /build/openapi/aidash.json
RUN npm run generate:api && npx tsc -b && npx vite build

FROM nginxinc/nginx-unprivileged:1.31-alpine3.24 AS frontend
COPY --from=web /build/web/dist /usr/share/nginx/html
COPY deploy/helm/aidash/files/default.conf /etc/nginx/conf.d/default.conf
EXPOSE 8080

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
