#!/usr/bin/env bash
# Keep real-process tests on this checkout's exact binary. A shared Cargo
# target can otherwise launch a different worktree's runtime during the test.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$PWD/.ignore/transaction-target"
export AIDASH_TEST_BINARY="$CARGO_TARGET_DIR/debug/aidash"
export RUSTC_WRAPPER=
export AIDASH_SECRET_TEST_PEER=transaction-test-only-peer-credential
# Distinct per-peer identities for the three- and sixteen-Node fixtures.
for transaction_peer in $(seq 1 15); do
  transaction_key=$(printf 'AIDASH_SECRET_TRANSACTION_%02d' "$transaction_peer")
  export "$transaction_key=transaction-acceptance-test-peer-$transaction_peer-only"
done
cargo test --locked --test transactions --test transaction_protocol -- --nocapture --test-threads=2 "$@"
if [[ $# == 0 ]]; then
  cargo test --locked --test scoped_remote_execution transaction_finalization -- --nocapture --test-threads=1
fi
