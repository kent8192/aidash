#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
distribution="${1:?Usage: scripts/test-cluster.sh kubernetes [platform|transactions|remote-memory|native-memory] [coordinator|participant|lifecycle]}"
case "$distribution" in kubernetes) ;; *) exit 2 ;; esac
profile="${2:-platform}"
case "$profile" in platform|transactions|remote-memory|native-memory) ;; *) exit 2 ;; esac
partition="${3:-}"
if [[ -n "$partition" ]]; then
  case "$profile" in
    transactions) case "$partition" in coordinator|participant|lifecycle) ;; *) exit 2 ;; esac ;;
    native-memory) case "$partition" in remote|evaluation|learning|learning-ui|all-ui) ;; *) exit 2 ;; esac ;;
    *) exit 2 ;;
  esac
fi
cluster="aidash-ci-$(date +%s)-$$"
# Parallel profiles must never replace a running, code-signed tool binary or
# share partially downloaded archives and diagnostic files.
tools_dir="$PWD/.ignore/platform/$cluster-tools"
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

export KUBECONFIG="$PWD/.ignore/platform/$cluster.kubeconfig"
created=false
cleanup() {
  if "$created"; then
    kind delete cluster --name "$cluster"
  fi
}
trap cleanup EXIT
curl -fsSL "https://kind.sigs.k8s.io/dl/v0.30.0/kind-$os-$arch" -o "$tools_dir/kind"
curl -fsSL "https://kind.sigs.k8s.io/dl/v0.30.0/kind-$os-$arch.sha256sum" -o "$tools_dir/kind.sha256"
python3 - "$tools_dir/kind" "$tools_dir/kind.sha256" <<'PY'
import hashlib, pathlib, sys
assert hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() == pathlib.Path(sys.argv[2]).read_text().split()[0]
PY
chmod +x "$tools_dir/kind"
created=true
kind create cluster --name "$cluster" --kubeconfig "$KUBECONFIG" --image kindest/node:v1.34.0@sha256:7416a61b42b1662ca6ca89f02028ac133a309a2a30ba309614e8ec94d976dc5a --wait 180s
helm lint deploy/helm/aidash --set node.id=aidash://acceptance --set existingSecret=acceptance
export AIDASH_CLUSTER_SOURCE_MANIFEST="$PWD/.ignore/platform/$cluster-source.json"
python3 - <<'SOURCE_MANIFEST'
import hashlib,json,os,pathlib,subprocess,datetime
files=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z']).decode().split('\0')
files={p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in sorted(set(files)) if p and pathlib.Path(p).is_file()}
manifest={'git_sha':subprocess.check_output(['git','rev-parse','HEAD']).decode().strip(),'dirty':bool(subprocess.check_output(['git','status','--porcelain'])),'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'file_hashes':files}
manifest['source_digest']=hashlib.sha256(json.dumps(files,sort_keys=True).encode()).hexdigest()
pathlib.Path(os.environ['AIDASH_CLUSTER_SOURCE_MANIFEST']).write_text(json.dumps(manifest,indent=2)+'\n')
SOURCE_MANIFEST
backend_image="${AIDASH_CLUSTER_IMAGE:-aidash:cluster-acceptance}"
images=("$backend_image")
if [[ "$profile" == transactions || "$profile" == remote-memory || "$profile" == native-memory ]]; then
  docker build --build-arg CARGO_PROFILE=dev --target dev-backend -t "$backend_image" .
else
  docker build --build-arg CARGO_PROFILE=dev -t "$backend_image" .
  docker build --build-arg CARGO_PROFILE=dev --target frontend -t aidash-frontend:cluster-acceptance .
  images+=(aidash-frontend:cluster-acceptance)
fi
postgres_image="${AIDASH_CLUSTER_POSTGRES_IMAGE:-aidash-postgres:17-pg-jsonschema-0.3.4}"
docker build -f deploy/postgres/Dockerfile -t "$postgres_image" .
kind load docker-image "${images[@]}" --name "$cluster"
kind load docker-image "$postgres_image" --name "$cluster"
if [[ "$profile" == remote-memory || "$profile" == native-memory ]]; then
  docker run --rm --network none --entrypoint manage "$backend_image" diagnostics remote-memory > "$tools_dir/remote-memory-queries.json"
  if [[ "$profile" == native-memory ]]; then runner=scripts/native_memory_cluster_acceptance.py;
  else runner=scripts/remote_memory_cluster_acceptance.py; fi
  native_selection=()
  if [[ "$profile" == native-memory && -n "$partition" ]]; then
    if [[ "$partition" == learning-ui ]]; then native_selection=(--phase learning --live-ui);
    elif [[ "$partition" == all-ui ]]; then native_selection=(--phase all --live-ui);
    else native_selection=(--phase "$partition"); fi
  fi
  python3 "$runner" --kubeconfig "$KUBECONFIG" --distribution "$distribution" --image "$backend_image" --postgres-image "$postgres_image" --queries "$tools_dir/remote-memory-queries.json" "${native_selection[@]}"
elif [[ "$profile" == transactions ]]; then
  docker run --rm --network none --entrypoint manage "$backend_image" diagnostics acceptance > "$tools_dir/transaction-queries.json"
  selection=()
  if [[ -n "$partition" ]]; then selection+=(--partition "$partition"); fi
  python3 scripts/transaction_cluster_acceptance.py --kubeconfig "$KUBECONFIG" --distribution "$distribution" --image "$backend_image" --postgres-image "$postgres_image" --queries "$tools_dir/transaction-queries.json" "${selection[@]}"
else
  python3 scripts/cluster_acceptance.py --kubeconfig "$KUBECONFIG" --distribution "$distribution" --image "$backend_image" --frontend-image aidash-frontend:cluster-acceptance --postgres-image "$postgres_image" --dashboard
fi
