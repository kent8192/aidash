#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
distribution="${1:?Usage: scripts/test-cluster.sh kubernetes|k3s}"
case "$distribution" in kubernetes|k3s) ;; *) exit 2 ;; esac
tools_dir="$PWD/.ignore/platform/cluster-tools"
mkdir -p "$tools_dir"
export PATH="$tools_dir:$PATH"
os=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$(uname -m)" in aarch64|arm64) arch=arm64 ;; x86_64) arch=amd64 ;; *) exit 2 ;; esac

# Install pinned, checksum-verified tools inside this task's ignored directory.
curl -fsSL "https://get.helm.sh/helm-v4.1.4-$os-$arch.tar.gz" -o "$tools_dir/helm.tar.gz"
curl -fsSL "https://get.helm.sh/helm-v4.1.4-$os-$arch.tar.gz.sha256sum" -o "$tools_dir/helm.sha256"
python3 - "$tools_dir/helm.tar.gz" "$tools_dir/helm.sha256" <<'PY'
import hashlib, pathlib, sys
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == pathlib.Path(sys.argv[2]).read_text().split()[0]
PY
tar -xzf "$tools_dir/helm.tar.gz" -C "$tools_dir" "$os-$arch/helm"
cp "$tools_dir/$os-$arch/helm" "$tools_dir/helm"
curl -fsSL "https://dl.k8s.io/release/v1.34.0/bin/$os/$arch/kubectl" -o "$tools_dir/kubectl"
curl -fsSL "https://dl.k8s.io/release/v1.34.0/bin/$os/$arch/kubectl.sha256" -o "$tools_dir/kubectl.sha256"
python3 - "$tools_dir/kubectl" "$tools_dir/kubectl.sha256" <<'PY'
import hashlib, pathlib, sys
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == pathlib.Path(sys.argv[2]).read_text().split()[0]
PY
chmod +x "$tools_dir/kubectl"

cluster="aidash-ci-$(date +%s)-$$"
export KUBECONFIG="$PWD/.ignore/platform/$cluster.kubeconfig"
created=false
cleanup() {
  if "$created"; then
    if [[ "$distribution" == kubernetes ]]; then kind delete cluster --name "$cluster";
    else k3d cluster delete "$cluster"; fi
  fi
}
trap cleanup EXIT
if [[ "$distribution" == kubernetes ]]; then
  curl -fsSL "https://kind.sigs.k8s.io/dl/v0.30.0/kind-$os-$arch" -o "$tools_dir/kind"
  curl -fsSL "https://kind.sigs.k8s.io/dl/v0.30.0/kind-$os-$arch.sha256sum" -o "$tools_dir/kind.sha256"
  python3 - "$tools_dir/kind" "$tools_dir/kind.sha256" <<'PY'
import hashlib, pathlib, sys
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == pathlib.Path(sys.argv[2]).read_text().split()[0]
PY
  chmod +x "$tools_dir/kind"
  created=true
  kind create cluster --name "$cluster" --kubeconfig "$KUBECONFIG" --image kindest/node:v1.34.0@sha256:7416a61b42b1662ca6ca89f02028ac133a309a2a30ba309614e8ec94d976dc5a --wait 180s
else
  curl -fsSL "https://github.com/k3d-io/k3d/releases/download/v5.9.0/k3d-$os-$arch" -o "$tools_dir/k3d"
  curl -fsSL https://github.com/k3d-io/k3d/releases/download/v5.9.0/checksums.txt -o "$tools_dir/k3d.sha256"
  python3 - "$tools_dir/k3d" "$tools_dir/k3d.sha256" "k3d-$os-$arch" <<'PY'
import hashlib, pathlib, sys
expected = next(line.split()[0] for line in pathlib.Path(sys.argv[2]).read_text().splitlines() if pathlib.Path(line.split()[-1]).name == sys.argv[3])
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == expected
PY
  chmod +x "$tools_dir/k3d"
  created=true
  k3d cluster create "$cluster" --image rancher/k3s:v1.34.11-k3s1 --kubeconfig-update-default=false --kubeconfig-switch-context=false --wait
  k3d kubeconfig get "$cluster" > "$KUBECONFIG"
fi
helm lint deploy/helm/aidash --set node.id=aidash://acceptance --set existingSecret=acceptance
docker build --build-arg CARGO_PROFILE=dev -t aidash:cluster-acceptance .
docker build --build-arg CARGO_PROFILE=dev --target frontend -t aidash-frontend:cluster-acceptance .
postgres_image=aidash-postgres:17-pg-jsonschema-0.3.4
docker build -f deploy/postgres/Dockerfile -t "$postgres_image" .
if [[ "$distribution" == kubernetes ]]; then
  kind load docker-image aidash:cluster-acceptance aidash-frontend:cluster-acceptance --name "$cluster"
  kind load docker-image "$postgres_image" --name "$cluster"
else
  k3d image import aidash:cluster-acceptance aidash-frontend:cluster-acceptance --cluster "$cluster"
  k3d image import "$postgres_image" --cluster "$cluster"
fi
python3 scripts/cluster_acceptance.py --kubeconfig "$KUBECONFIG" --distribution "$distribution" --image aidash:cluster-acceptance --frontend-image aidash-frontend:cluster-acceptance --postgres-image "$postgres_image" --dashboard
