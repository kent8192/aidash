#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
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
