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
coverage=false
partition=all
shard=
capability_test_root=
shard_runner=
cleanup() {
  [[ -z "$capability_test_root" ]] || rm -rf "$capability_test_root"
  [[ -z "$shard_runner" ]] || rm -f "$shard_runner"
}
trap cleanup EXIT
while (($#)); do
  case "$1" in
    --coverage) coverage=true; shift ;;
    --partition) partition="${2:?a partition name is required}"; shift 2 ;;
    --shard) shard="${2:?a shard number is required}"; shift 2 ;;
    *) echo 'Usage: scripts/test-rust.sh [--coverage] [--partition NAME] [--shard 1|2]' >&2; exit 2 ;;
  esac
done
if [[ -n "$shard" ]]; then
  [[ "$shard" == 1 || "$shard" == 2 ]] || { echo 'Rust test shard must be 1 or 2' >&2; exit 2; }
  export AIDASH_RUST_TEST_SHARD="$shard"
  export AIDASH_RUST_TEST_SHARD_RUNNER="$PWD/scripts/rust-test-shard.py"
  # Cargo runner strings split on spaces. An owned wrapper in /tmp also handles
  # workspace paths containing spaces without changing user Cargo configuration.
  shard_runner=$(mktemp /tmp/aidash-rust-shard.XXXXXX)
  cat > "$shard_runner" <<'SHARD_RUNNER'
#!/bin/sh
exec python3 "$AIDASH_RUST_TEST_SHARD_RUNNER" "$@"
SHARD_RUNNER
  chmod +x "$shard_runner"
  rust_host=$(rustc -vV | sed -n 's/^host: //p')
  runner_key="CARGO_TARGET_$(printf '%s' "$rust_host" | tr '[:lower:]-' '[:upper:]_')_RUNNER"
  export "$runner_key=$shard_runner"
fi
test_args=(--workspace --all-targets)
if [[ "$partition" != all ]]; then
  test_args=()
  partition_args=(--partition "$partition")
  "$coverage" && partition_args+=(--coverage)
  while IFS= read -r argument; do
    test_args+=("$argument")
  done < <(python3 scripts/rust-test-partitions.py "${partition_args[@]}")
  # A process-substitution failure must never fall back to unscoped cargo test.
  ((${#test_args[@]})) || exit 2
fi
if [[ "$partition" != foundation ]]; then
  # Default file Tools and explicit Host packages need an admitted deployment.
  # This profile owns only disposable test storage and never enables a host runner.
  if [[ -z "${AIDASH_CAPABILITY_PROFILE:-}" ]]; then
    capability_test_root=$(mktemp -d "${TMPDIR:-/tmp}/aidash-rust-profile.XXXXXX")
    export AIDASH_CAPABILITY_PROFILE="$capability_test_root/profile.json"
    python3 - "$capability_test_root" <<'PY_PROFILE'
import json
import pathlib
import sys
root = pathlib.Path(sys.argv[1])
(root / "profile.json").write_text(json.dumps({"admission": True, "storage": str(root / "storage"), "outbound_origins": ["https://example.com"]}))
PY_PROFILE
  fi
  scripts/build-test-postgres.sh
fi
if "$coverage"; then
    mkdir -p coverage
    coverage_target_dir="${AIDASH_COVERAGE_TARGET_DIR:-$PWD/target/llvm-cov}"
    coverage_file=coverage/rust.lcov
    [[ "$partition" == all ]] || coverage_file="coverage/rust-$partition.lcov"
    [[ -z "$shard" ]] || coverage_file="${coverage_file%.lcov}-$shard.lcov"
    CARGO_TARGET_DIR="$coverage_target_dir" cargo llvm-cov --locked "${test_args[@]}" --lcov \
      --ignore-filename-regex '(/tests/|/migrations/|/([^/]*_)?tests\.rs$)' --output-path "$coverage_file"
    test -s "$coverage_file"
else
  cargo test --locked "${test_args[@]}"
fi
