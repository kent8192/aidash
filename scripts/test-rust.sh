#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose up -d --wait
export AIDASH_TEST_DATABASE_URL="${AIDASH_TEST_DATABASE_URL:-postgres://aidash:aidash-local@127.0.0.1:${AIDASH_POSTGRES_PORT:-54370}/aidash_test}"
export AIDASH_SECRET_TEST_PEER="${AIDASH_SECRET_TEST_PEER:-local-peer-regression-test-token-0123456789}"
export AIDASH_TEST_NATS_URL="${AIDASH_TEST_NATS_URL:-nats://127.0.0.1:${AIDASH_NATS_PORT:-42270}}"
export AIDASH_TEST_QDRANT_URL="${AIDASH_TEST_QDRANT_URL:-http://127.0.0.1:${AIDASH_QDRANT_PORT:-63370}}"
export AIDASH_SECRET_TEST_QDRANT="${AIDASH_SECRET_TEST_QDRANT:-local-semantic-vector-fixture-key-0123456789}"
export RUSTC_WRAPPER=
case "${1:-}" in
  '') cargo test --locked --workspace --all-targets -- --include-ignored ;;
  --coverage)
    mkdir -p coverage
    cargo llvm-cov --locked --workspace --all-targets --lcov \
      --ignore-filename-regex '(/tests/|/migration/)' --output-path coverage/rust.lcov \
      -- --include-ignored
    test -s coverage/rust.lcov
    ;;
  *) echo 'Usage: scripts/test-rust.sh [--coverage]' >&2; exit 2 ;;
esac
