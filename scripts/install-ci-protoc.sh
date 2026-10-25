#!/usr/bin/env bash
# Native reinhardt-grpc dependencies compile their own Protocol Buffers sources.
set -euo pipefail
if command -v protoc >/dev/null 2>&1; then
  protoc --version
  exit 0
fi
if [[ ${GITHUB_ACTIONS:-} != true || $(uname -s) != Linux ]]; then
  echo "Install the Protocol Buffers compiler (protoc) before native Rust builds." >&2
  exit 1
fi
"$(dirname "$0")/configure-ci-apt.sh"
sudo apt-get update
sudo apt-get install -y --no-install-recommends protobuf-compiler
protoc --version
