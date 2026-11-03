#!/usr/bin/env bash
# Instrumented coverage builds and per-test PostgreSQL databases share the runner's
# root disk. Federation shards ran out of space inside the PostgreSQL container
# ("could not extend file ...: No space left on device"), so remove preinstalled
# toolchains that no Rust test job uses before building.
set -euo pipefail
if [[ ${GITHUB_ACTIONS:-} != true || $(uname -s) != Linux ]]; then
  echo "Only GitHub-hosted Linux runners are cleaned; nothing to do here." >&2
  exit 0
fi
df -h /
sudo rm -rf \
  /usr/share/dotnet \
  /usr/share/swift \
  /usr/local/lib/android \
  /usr/local/.ghcup \
  /opt/ghc \
  /opt/hostedtoolcache/CodeQL
df -h /
