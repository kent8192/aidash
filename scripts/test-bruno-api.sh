#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$project_root"
export RUSTC_WRAPPER=
cargo build --locked -p aidash-server --bin aidash
binary=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')
python3 "$project_root/scripts/test-bruno-api.py" --binary "$binary" "$@"
