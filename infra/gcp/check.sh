#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
terraform fmt -check -recursive infra/gcp
terraform fmt -check -recursive infra/delivery
for root in infra/gcp/bootstrap infra/gcp/environments infra/gcp/modules/*/ infra/delivery/github infra/delivery/cloudflare infra/delivery/preview-domain infra/delivery/projects; do
  root="${root%/}"
  terraform -chdir="$root" init -backend=false -input=false -no-color
  terraform -chdir="$root" validate -no-color
  terraform -chdir="$root" test -no-color
done
python3 -m unittest discover -s infra/gcp/tests -v
python3 -m unittest discover -s infra/gcp/bootstrap/tests -v
python3 -m unittest discover -s infra/delivery/tests -v
python3 -m unittest discover -s infra/delivery/control/tests -v
python3 -m unittest discover -s infra/delivery/gcp -p "test_*.py" -v
cargo fmt --manifest-path infra/gcp/observer/Cargo.toml -- --check
cargo test --locked --manifest-path infra/gcp/observer/Cargo.toml
python3 infra/gcp/tests/postgres.py
# Every edge image build input is pinned, so the check's result depends only on
# these files. A pull request that leaves them as on its base skips the rebuild,
# which needs the external Ubuntu snapshot service; anything else runs it.
edge_inputs=(infra/gcp/helm/environment/edge.Dockerfile infra/gcp/helm/environment/files scripts/test-gcp-edge.sh)
if [[ -n "${GITHUB_BASE_REF:-}" ]] &&
  git fetch --quiet --no-tags --depth=1 origin "refs/heads/$GITHUB_BASE_REF" &&
  git diff --quiet FETCH_HEAD HEAD -- "${edge_inputs[@]}"; then
  echo "Edge admission inputs match $GITHUB_BASE_REF; skipping the edge image check."
else
  bash scripts/test-gcp-edge.sh
fi
python3 scripts/test_helm_execution.py
