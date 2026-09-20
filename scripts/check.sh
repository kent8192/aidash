#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose up -d --wait
export AIDASH_TEST_DATABASE_URL="${AIDASH_TEST_DATABASE_URL:-postgres://aidash:aidash-local@127.0.0.1:54370/aidash_test}"
export AIDASH_SECRET_TEST_PEER="${AIDASH_SECRET_TEST_PEER:-local-peer-regression-test-token-0123456789}"
RUSTC_WRAPPER= cargo test --locked --all-targets -- --include-ignored
RUSTC_WRAPPER= cargo build --locked
npm ci --prefix web
scripts/generate-api.sh
if [[ "${CI:-}" == "true" ]]; then
  git diff --exit-code -- openapi/aidash.json web/src/generated
fi
npm run build --prefix web
(cd web && npm exec -- playwright install --with-deps chromium)
AIDASH_TEST_BINARY=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')
python3 scripts/golden_path.py --binary "$AIDASH_TEST_BINARY" --dashboard
