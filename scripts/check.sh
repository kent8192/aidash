#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo metadata --locked --format-version 1 --manifest-path enterprise/Cargo.toml | python3 scripts/check-workspace-boundaries.py --workspace enterprise
python3 scripts/check-enterprise-alignment.py
scripts/test-rust.sh
scripts/test-acceptance.sh
