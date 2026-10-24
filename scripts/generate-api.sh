#!/usr/bin/env bash
# Export the server-owned contract without connecting to PostgreSQL or NATS.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p openapi
schema=$(mktemp)
trap 'rm -f "$schema"' EXIT
# A caller that already built `manage`, such as the release acceptance run,
# names it instead of building a debug copy.
if [[ -n "${AIDASH_MANAGE_BINARY:-}" ]]; then
  "$AIDASH_MANAGE_BINARY" openapi >"$schema"
else
  RUSTC_WRAPPER='' cargo run --locked --quiet -p aidash-server --bin manage -- openapi >"$schema"
fi
mv "$schema" openapi/aidash.json
npm run generate:api --prefix web
