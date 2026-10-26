#!/usr/bin/env bash
# Export the server-owned contract without connecting to PostgreSQL or NATS.
# AIDASH_OPENAPI_DESCRIPTION_ONLY skips the dashboard client so that the
# Community export can compare two descriptions without Node dependencies.
set -euo pipefail
cd "$(dirname "$0")/.."
output="${AIDASH_OPENAPI_OUTPUT:-openapi/aidash.json}"
mkdir -p "$(dirname "$output")"
schema=$(mktemp)
trap 'rm -f "$schema"' EXIT
# A caller that already built `manage`, such as the release acceptance run,
# names it instead of building a debug copy.
if [[ -n "${AIDASH_MANAGE_BINARY:-}" ]]; then
  "$AIDASH_MANAGE_BINARY" openapi >"$schema"
else
  RUSTC_WRAPPER='' cargo run --locked --quiet -p aidash-server --bin manage -- openapi >"$schema"
fi
mv "$schema" "$output"
if [[ -z "${AIDASH_OPENAPI_DESCRIPTION_ONLY:-}" ]]; then
  npm run generate:api --prefix web
fi
