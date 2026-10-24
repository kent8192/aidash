# syntax=docker/dockerfile:1
# Build with enterprise/auth/frontend as the context; no Cloud workspace is required.
FROM rust:1.96-bookworm AS wasm
ARG CARGO_BUILD_JOBS=2
WORKDIR /app
ARG WASM_PACK_VERSION=0.15.0
RUN cargo install wasm-pack --version "$WASM_PACK_VERSION" --locked && \
    rustup target add wasm32-unknown-unknown
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/local/cargo/wasm-pack-cache \
    --mount=type=cache,target=/app/target \
    WASM_PACK_CACHE=/usr/local/cargo/wasm-pack-cache \
    wasm-pack build . --target web --out-dir pkg --out-name aidash_auth_web -- --locked

FROM node:24.12.0-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts
COPY index.html tsconfig.json vite.config.ts ./
COPY src ./src
COPY --from=wasm /app/pkg ./pkg
RUN npm run build:assets

FROM nginxinc/nginx-unprivileged:1.31-alpine3.24
ENV AUTHENTICATION_BACKEND_URL=http://authentication:8081
ENV NGINX_ENVSUBST_FILTER=AUTHENTICATION_BACKEND_URL
ENV NGINX_ENVSUBST_OUTPUT_DIR=/tmp/nginx-conf
USER root
RUN sed -i 's|/etc/nginx/conf.d/\*.conf|/tmp/nginx-conf/*.conf|' /etc/nginx/nginx.conf && \
    rm /etc/nginx/conf.d/default.conf
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
COPY --chmod=755 scripts/nginx-entrypoint.sh /usr/local/bin/auth-web-entrypoint
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/auth-web-entrypoint"]
CMD ["nginx", "-g", "daemon off;"]
EXPOSE 8080
