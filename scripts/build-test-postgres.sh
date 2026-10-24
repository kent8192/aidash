#!/usr/bin/env bash
# Build the disposable Testcontainers image without Compose's database init script.
set -euo pipefail
cd "$(dirname "$0")/.."
docker build --target test -f deploy/postgres/Dockerfile \
  -t aidash-orm-test-postgres:17-pg-jsonschema-0.3.4 .
