#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

set -euo pipefail
cd "$(dirname "$0")/../../.."
terraform fmt -check -recursive enterprise/infra/gcp
terraform fmt -check -recursive enterprise/infra/delivery
for root in enterprise/infra/gcp/bootstrap enterprise/infra/gcp/environments enterprise/infra/gcp/modules/*/ enterprise/infra/gcp/restore-drill enterprise/infra/delivery/github enterprise/infra/delivery/cloudflare enterprise/infra/delivery/preview-domain enterprise/infra/delivery/projects; do
  root="${root%/}"
  terraform -chdir="$root" init -backend=false -input=false -no-color
  terraform -chdir="$root" validate -no-color
  terraform -chdir="$root" test -no-color
done
python3 -m unittest discover -s enterprise/infra/gcp/tests -v
python3 -m unittest discover -s enterprise/infra/gcp/bootstrap/tests -v
python3 -m unittest discover -s enterprise/infra/delivery/tests -v
python3 -m unittest discover -s enterprise/infra/delivery/control/tests -v
python3 -m unittest discover -s enterprise/infra/delivery/gcp -p "test_*.py" -v
cargo fmt --manifest-path enterprise/infra/gcp/observer/Cargo.toml -- --check
cargo test --locked --manifest-path enterprise/infra/gcp/observer/Cargo.toml
python3 enterprise/infra/gcp/tests/postgres.py
# Every edge image build input is pinned, so the check's result depends only on
# these files. A pull request that leaves them as on its base skips the rebuild,
# which needs the external Ubuntu snapshot service; anything else runs it.
edge_inputs=(enterprise/infra/gcp/helm/environment/edge.Dockerfile enterprise/infra/gcp/helm/environment/files enterprise/scripts/test-gcp-edge.sh)
if [[ -n "${GITHUB_BASE_REF:-}" ]] &&
  git fetch --quiet --no-tags --depth=1 origin "refs/heads/$GITHUB_BASE_REF" &&
  git diff --quiet FETCH_HEAD HEAD -- "${edge_inputs[@]}"; then
  echo "Edge admission inputs match $GITHUB_BASE_REF; skipping the edge image check."
else
  bash enterprise/scripts/test-gcp-edge.sh
fi
python3 scripts/test_helm_execution.py
