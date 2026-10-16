#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/check-license-declarations.py
scripts/test-rust.sh
scripts/test-acceptance.sh
