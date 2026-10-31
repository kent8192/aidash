#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m unittest discover -s scripts -p test_cli_backend_examples.py
scripts/test-rust.sh
scripts/test-acceptance.sh
