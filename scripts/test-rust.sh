#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
# These immutable, local-only credentials are inherited when Cargo starts each
# test binary; dynamically mapped service endpoints come from TestEnvironment.
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
export AIDASH_SECRET_GRAPH_THIRD=local-third-graph-peer-token-9876543210
# Distinct per-peer identities for the three- and sixteen-Node fixtures.
for transaction_peer in $(seq 1 15); do
  transaction_key=$(printf 'AIDASH_SECRET_TRANSACTION_%02d' "$transaction_peer")
  export "$transaction_key=transaction-acceptance-test-peer-$transaction_peer-only"
done
export AIDASH_SECRET_TEST_QDRANT=local-semantic-vector-fixture-key-0123456789
case "${1:-}" in
  '') cargo test --locked --workspace --all-targets ;;
  --coverage)
    mkdir -p coverage
    coverage_target_dir="${AIDASH_COVERAGE_TARGET_DIR:-$PWD/target/llvm-cov}"
    CARGO_TARGET_DIR="$coverage_target_dir" cargo llvm-cov --locked --workspace --all-targets --lcov \
      --ignore-filename-regex '(/tests/|/migration/)' --output-path coverage/rust.lcov
    test -s coverage/rust.lcov
    ;;
  *) echo 'Usage: scripts/test-rust.sh [--coverage]' >&2; exit 2 ;;
esac
