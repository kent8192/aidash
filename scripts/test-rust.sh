#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
# These immutable, local-only credentials are inherited when Cargo starts each
# test binary; dynamically mapped service endpoints come from TestEnvironment.
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
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
