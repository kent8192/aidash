#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$project_root"
export RUSTC_WRAPPER=
build_report=$(mktemp /tmp/aidash-bruno-build.XXXXXX)
trap 'rm -f "$build_report"' EXIT
cargo build --locked -p aidash-server --bin aidash --bin manage --message-format=json > "$build_report"
# Use Cargo's emitted artifact paths so custom target/build directories cannot
# silently select a binary built from a different worktree.
manage=$(python3 - "$build_report" manage <<'PY'
import json, sys
artifacts = [json.loads(line) for line in open(sys.argv[1])]
print(next(item["executable"] for item in artifacts if item.get("reason") == "compiler-artifact" and item["target"]["name"] == sys.argv[2] and item.get("executable")))
PY
)
python3 "$project_root/scripts/test-bruno-api.py" --binary "$manage" --manage "$manage" "$@"
