#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/test-rust.sh
scripts/test-acceptance.sh
