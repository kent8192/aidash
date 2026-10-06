#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ${GITHUB_ACTIONS:-} == true && $(uname -s) == Linux ]]; then
  scripts/configure-ci-apt.sh
  timeout 10m npm exec --prefix web -- playwright install --with-deps chromium
else
  npm exec --prefix web -- playwright install --with-deps chromium
fi
