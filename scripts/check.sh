#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose up -d --wait
export AIDASH_TEST_DATABASE_URL="${AIDASH_TEST_DATABASE_URL:-postgres://aidash:aidash-local@127.0.0.1:54370/aidash_test}"
RUSTC_WRAPPER= cargo test --locked --all-targets -- --include-ignored
RUSTC_WRAPPER= cargo build --locked
npm ci --prefix web
npm run build --prefix web
(cd web && npm exec -- playwright install --with-deps chromium)
AIDASH_TEST_BINARY=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')
python3 scripts/golden_path.py --binary "$AIDASH_TEST_BINARY" --dashboard
