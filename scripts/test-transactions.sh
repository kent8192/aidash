#!/usr/bin/env bash
# Keep real-process tests on this checkout's exact binary. A shared Cargo
# target can otherwise launch a different worktree's runtime during the test.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-test-postgres.sh
export CARGO_TARGET_DIR="$PWD/private/transaction-target"
export AIDASH_TEST_BINARY="$CARGO_TARGET_DIR/debug/aidash"
export RUSTC_WRAPPER=
cargo test --locked -p aidash-server --test transactions --test transaction_protocol -- --nocapture --test-threads=2 "$@"
if [[ $# == 0 ]]; then
  cargo test --locked -p aidash-server --test scoped_remote_execution transaction_finalization -- --nocapture --test-threads=1
fi
