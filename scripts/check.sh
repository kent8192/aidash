#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m unittest scripts/test_check_licensing.py
python3 scripts/check-licensing.py scope
python3 scripts/check-licensing.py expression
python3 scripts/check-licensing.py text
python3 scripts/check-licensing.py params
python3 scripts/check-licensing.py boundary
python3 scripts/check-licensing.py history --base "${AIDASH_LICENSING_BASE:-HEAD^1}"
if [ -n "${AIDASH_LICENSING_INVENTORY:-}" ]; then
	python3 scripts/check-licensing.py attribution --inventory "$AIDASH_LICENSING_INVENTORY"
fi
if [ -n "${AIDASH_LICENSING_IMAGE:-}" ]; then
	python3 scripts/check-licensing.py artifact --image "$AIDASH_LICENSING_IMAGE"
fi
scripts/test-rust.sh
scripts/test-acceptance.sh
