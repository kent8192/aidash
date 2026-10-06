#!/usr/bin/env bash
# Configure only ephemeral Linux Actions runners; leave local sources alone.
set -euo pipefail
if [[ ${GITHUB_ACTIONS:-} != true || $(uname -s) != Linux ]]; then exit 0; fi
# Hosted Ubuntu resolves package URLs through apt-mirrors.txt as well as direct
# entries. See docs/operations/ci-coverage.md for evidence and removal conditions.
shopt -s nullglob
for source in /etc/apt/apt-mirrors*.txt /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
  if [[ -f $source ]]; then
    sudo sed -Ei 's|https?://(azure\.)?archive\.ubuntu\.com/ubuntu|https://archive.ubuntu.com/ubuntu|g' "$source"
  fi
done
sudo tee /etc/apt/apt.conf.d/99-aidash-ci-timeouts >/dev/null <<'APT'
Acquire::Retries "3";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
APT
