#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ${GITHUB_ACTIONS:-} == true && $(uname -s) == Linux ]]; then
  # Hosted Ubuntu's Azure HTTP mirror can stall after InRelease redirects,
  # while package indexes still use the original URI. Use one HTTPS origin.
  shopt -s nullglob
  for source in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    if [[ -f $source ]]; then
      sudo sed -i 's|https\?://azure.archive.ubuntu.com/ubuntu|https://archive.ubuntu.com/ubuntu|g' "$source"
    fi
  done
  sudo tee /etc/apt/apt.conf.d/99-aidash-ci-timeouts >/dev/null <<'APT'
Acquire::Retries "3";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
APT
  timeout 10m npm exec --prefix web -- playwright install --with-deps chromium
else
  npm exec --prefix web -- playwright install --with-deps chromium
fi
