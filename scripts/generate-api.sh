#!/usr/bin/env bash
# Export the server-owned contract without connecting to PostgreSQL or NATS.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p openapi
schema=$(mktemp)
trap 'rm -f "$schema"' EXIT
RUSTC_WRAPPER= cargo run --locked --quiet -- openapi > "$schema"
mv "$schema" openapi/aidash.json
npm run generate:api --prefix web
