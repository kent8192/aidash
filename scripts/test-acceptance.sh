#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose up -d --wait
export RUSTC_WRAPPER=
cargo build --locked --bin aidash
npm ci --prefix web
# prebuild generates the ignored Rust-owned OpenAPI contract and TypeScript client.
npm run build --prefix web
(cd web && npm exec -- playwright install --with-deps chromium)
AIDASH_TEST_BINARY=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')
python3 scripts/golden_path.py --binary "$AIDASH_TEST_BINARY" --dashboard
