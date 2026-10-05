#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
# Match hosted runner concurrency and bound shared service load on large hosts.
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
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
coverage=false
partition=all
while (($#)); do
  case "$1" in
    --coverage) coverage=true; shift ;;
    --partition) partition="${2:?a partition name is required}"; shift 2 ;;
    *) echo 'Usage: scripts/test-rust.sh [--coverage] [--partition NAME]' >&2; exit 2 ;;
  esac
done
test_args=(--workspace --all-targets)
if [[ "$partition" != all ]]; then
  test_args=()
  while IFS= read -r argument; do
    test_args+=("$argument")
  done < <(python3 scripts/rust-test-partitions.py --partition "$partition")
  # A process-substitution failure must never fall back to unscoped cargo test.
  ((${#test_args[@]})) || exit 2
fi
if [[ "$partition" != foundation ]]; then
  scripts/build-test-postgres.sh
fi
if "$coverage"; then
    mkdir -p coverage
    coverage_target_dir="${AIDASH_COVERAGE_TARGET_DIR:-$PWD/target/llvm-cov}"
    coverage_file=coverage/rust.lcov
    [[ "$partition" == all ]] || coverage_file="coverage/rust-$partition.lcov"
    CARGO_TARGET_DIR="$coverage_target_dir" cargo llvm-cov --locked "${test_args[@]}" --lcov \
      --ignore-filename-regex '(/tests/|/migrations/)' --output-path "$coverage_file"
    test -s "$coverage_file"
else
  cargo test --locked "${test_args[@]}"
fi
