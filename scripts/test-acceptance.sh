#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export COMPOSE_FILE="${COMPOSE_FILE:-compose.yaml}:server/src/apps/execution/tests/fixtures/acceptance.compose.yaml"
docker compose up -d --wait
export RUSTC_WRAPPER=
# Two Nodes, their workers, PostgreSQL and Chromium share one CI runner. A
# debug build spends most of a Tenant subject's request in unoptimized
# authorization and serialization and misses the Golden Path's budgets (#404),
# so the run uses the release build that ships.
cargo build --locked --release -p aidash-server --bin aidash --bin manage
release=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/release")')
npm ci --prefix web
# prebuild generates the ignored Rust-owned OpenAPI contract and TypeScript client.
AIDASH_MANAGE_BINARY="$release/manage" npm run build --prefix web
scripts/install-browser.sh
# No argument runs the golden path with its @golden-path browser specs; --browser-suite runs the rest of the browser suite after the setup alone.
python3 scripts/golden_path.py --binary "$release/aidash" "${@:---dashboard}"
