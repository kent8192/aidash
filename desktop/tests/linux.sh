#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
mkdir -p desktop/artifacts
node --input-type=module - <<'JS'
import { meshScene } from './web/tests/mesh-scene.mjs';
import { writeFileSync } from 'node:fs';
writeFileSync('desktop/artifacts/scene.json', JSON.stringify(meshScene()));
JS
docker build -t aidash-desktop-test:local -f desktop/tests/Dockerfile desktop/tests
docker run --rm \
  -v "$root/desktop:/workspace/desktop" \
  -v "$root/web/dist:/workspace/web/dist:ro" \
  -v aidash-desktop-cargo-registry:/usr/local/cargo/registry \
  -v aidash-desktop-linux-target:/build \
  -e CARGO_TARGET_DIR=/build \
  aidash-desktop-test:local bash -c \
  'cargo build --locked --manifest-path /workspace/desktop/src-tauri/Cargo.toml --features tauri/custom-protocol && dbus-run-session -- xvfb-run -a bash /workspace/desktop/tests/linux-session.sh'
