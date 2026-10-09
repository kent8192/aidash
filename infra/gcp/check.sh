#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
terraform fmt -check -recursive infra/gcp
for root in bootstrap environments modules/environment modules/credential-broker; do
  terraform -chdir="infra/gcp/$root" init -backend=false -input=false -no-color
  terraform -chdir="infra/gcp/$root" validate -no-color
  terraform -chdir="infra/gcp/$root" test -no-color
done
python3 -m unittest discover -s infra/gcp/tests -v
python3 -m unittest discover -s infra/gcp/bootstrap/tests -v
cargo fmt --manifest-path infra/gcp/observer/Cargo.toml -- --check
cargo test --locked --manifest-path infra/gcp/observer/Cargo.toml
python3 infra/gcp/tests/postgres.py
python3 infra/gcp/tests/nginx.py
