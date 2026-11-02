#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

set -euo pipefail
project_dir="$(cd "$(dirname "$0")/.." && pwd)"
wasm-pack build "$project_dir" --target web --out-dir dist-wasm --out-name aidash_marketplace_server --no-typescript --release -- --locked
python3 "$project_dir/../../scripts/package-standalone-pages.py" --project "$project_dir"
