FROM rust:1.96-bookworm AS wasm
ARG CARGO_BUILD_JOBS=2
RUN rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack --version 0.15.0 --locked
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN wasm-pack build --target web --out-dir pkg --out-name aidash_marketplace_web --release --locked

FROM node:24.12.0-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts
COPY index.html tsconfig.json vite.config.ts ./
COPY src ./src
COPY --from=wasm /app/pkg ./pkg
RUN npm run bundle

FROM nginxinc/nginx-unprivileged:1.31-alpine3.24
ENV MARKETPLACE_BACKEND_URL=http://marketplace:8082
ENV NGINX_ENVSUBST_FILTER=MARKETPLACE_BACKEND_URL
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 8080
