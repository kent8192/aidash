# Build from repository root. Enterprise and Community share one source tree.
FROM rust:1.96-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --manifest-path enterprise/Cargo.toml -p aidash-marketplace-server --locked
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/enterprise/target/release/aidash-marketplace-server /usr/local/bin/aidash-marketplace-server
USER 65532:65532
ENV AIDASH_MARKETPLACE_LISTEN=0.0.0.0:8082
EXPOSE 8082
ENTRYPOINT ["aidash-marketplace-server"]
