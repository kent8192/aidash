#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
terraform fmt -check -recursive infra/gcp
for root in infra/gcp/bootstrap infra/gcp/environments infra/gcp/modules/*/; do
  root="${root%/}"
  terraform -chdir="$root" init -backend=false -input=false -no-color
  terraform -chdir="$root" validate -no-color
  terraform -chdir="$root" test -no-color
done
python3 -m unittest discover -s infra/gcp/tests -v
python3 -m unittest discover -s infra/gcp/bootstrap/tests -v
cargo fmt --manifest-path infra/gcp/observer/Cargo.toml -- --check
cargo test --locked --manifest-path infra/gcp/observer/Cargo.toml
python3 infra/gcp/tests/postgres.py
bash scripts/test-gcp-edge.sh
python3 scripts/test_helm_execution.py
