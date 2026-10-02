#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export COMPOSE_FILE="${COMPOSE_FILE:-compose.yaml}:tests/fixtures/acceptance.compose.yaml"
docker compose up -d --wait
export RUSTC_WRAPPER=
cargo build --locked --bin aidash --example acceptance_queries
npm ci --prefix web
# prebuild generates the ignored Rust-owned OpenAPI contract and TypeScript client.
npm run build --prefix web
scripts/install-browser.sh
AIDASH_TEST_BINARY=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')
python3 scripts/golden_path.py --binary "$AIDASH_TEST_BINARY" --dashboard
