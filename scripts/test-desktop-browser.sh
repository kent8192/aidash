#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
# Uses disposable Testcontainers services and the installed web Playwright package.
scripts/build-test-postgres.sh
cargo test --locked -p aidash-server --test desktop_auth desktop_consent_in_chromium -- --ignored --exact --nocapture
