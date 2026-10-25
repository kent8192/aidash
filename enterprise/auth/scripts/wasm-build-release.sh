#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
wasm-pack build "$project_dir" --target web --out-dir dist-wasm --out-name aidash_auth_server --no-typescript --release -- --locked
python3 "$project_dir/../../scripts/package-standalone-pages.py" --project "$project_dir"
