#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
root=$PWD
state="$root/.ignore/local-k8s"
cluster=aidash-local
namespace=aidash-local
release=aidash
app_image=aidash:local
postgres_image=aidash-postgres:17-pg-jsonschema-0.3.4
export KUBECONFIG="$state/kubeconfig"

require_tools() {
  for tool in "$@"; do
    command -v "$tool" >/dev/null || { echo "Missing required tool: $tool" >&2; exit 1; }
  done
}

cluster_exists() {
  kind get clusters | grep -Fxq "$cluster"
}

check_port() {
  if ! [[ "$port" =~ ^[0-9]+$ ]] || (( port < 1 || port > 65535 )); then
    echo "AIDASH_K8S_PORT must be a port number from 1 to 65535" >&2
    exit 2
  fi
}

case "${1:-}" in
  up)
    require_tools kind kubectl helm docker python3 curl
    mkdir -p "$state"
    chmod 700 "$state"
    if [[ -f .env ]]; then
      set -a
      # Local .env follows the shell format used in the README.
      # shellcheck disable=SC1091 # Optional, user-provided file.
      source .env
      set +a
    fi
    export AIDASH_API_TOKEN=${AIDASH_API_TOKEN:-local-development-token}
    export AIDASH_LOCAL_POSTGRES_PASSWORD=${AIDASH_LOCAL_POSTGRES_PASSWORD:-aidash-local}
    export AIDASH_SECRET_TEST_QDRANT=${AIDASH_SECRET_TEST_QDRANT:-local-semantic-vector-fixture-key-0123456789}
    if (( ${#AIDASH_API_TOKEN} < 16 )); then
      echo "AIDASH_API_TOKEN must be at least 16 characters" >&2
      exit 2
    fi
    docker info >/dev/null
    if cluster_exists; then
      [[ -s "$KUBECONFIG" ]] || { echo "Cluster exists without $KUBECONFIG; refusing to use it" >&2; exit 1; }
      [[ -s "$state/port" ]] || { echo "Cluster exists without $state/port; run k8s-down before upgrading" >&2; exit 1; }
      port=$(cat "$state/port")
      if [[ -n "${AIDASH_K8S_PORT:-}" && "$AIDASH_K8S_PORT" != "$port" ]]; then
        echo "Cluster uses port $port; run k8s-down before changing AIDASH_K8S_PORT" >&2
        exit 2
      fi
    else
      port=${AIDASH_K8S_PORT:-8080}
      check_port
      python3 - "$port" <<'PY' > "$state/kind.yaml"
import socket
import sys

port = int(sys.argv[1])
with socket.socket() as listener:
    listener.bind(("127.0.0.1", port))
print(f"""kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: 30080
        hostPort: {port}
        listenAddress: 127.0.0.1
        protocol: TCP""")
PY
      kind create cluster --name "$cluster" --kubeconfig "$KUBECONFIG" --config "$state/kind.yaml" --wait 180s
      echo "$port" > "$state/port"
    fi
    kubectl cluster-info >/dev/null
    docker build -f deploy/postgres/Dockerfile -t "$postgres_image" .
    docker build -t "$app_image" .
    kind load docker-image "$postgres_image" "$app_image" --name "$cluster"
    kubectl create namespace "$namespace" --dry-run=client -o yaml | kubectl apply -f -
    python3 - <<'PY' | kubectl -n "$namespace" apply -f -
import json
import os
from urllib.parse import quote

secrets = {
    "DATABASE_URL": f"postgres://aidash:{quote(os.environ['AIDASH_LOCAL_POSTGRES_PASSWORD'], safe='')}@postgres:5432/aidash_a",
    "NATS_URL": "nats://nats:4222",
    "AIDASH_API_TOKEN": os.environ["AIDASH_API_TOKEN"],
}
secrets.update({key: value for key, value in os.environ.items() if key.startswith("AIDASH_SECRET_")})
for name, data in (
    ("aidash-local-infra", {"POSTGRES_PASSWORD": os.environ["AIDASH_LOCAL_POSTGRES_PASSWORD"], "QDRANT_API_KEY": os.environ["AIDASH_SECRET_TEST_QDRANT"]}),
    ("aidash-local-app", secrets),
):
    print("---")
    print(json.dumps({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": name}, "type": "Opaque", "stringData": data}))
PY
    kubectl -n "$namespace" apply -f deploy/local-k8s/infra.yaml
    for service in postgres nats qdrant; do
      kubectl -n "$namespace" rollout status "statefulset/$service" --timeout=300s
    done
    helm upgrade --install "$release" deploy/helm/aidash \
      --namespace "$namespace" \
      --set-string node.id=aidash://node-a \
      --set-string existingSecret=aidash-local-app \
      --set-string image.repository=aidash \
      --set-string image.tag=local \
      --set service.type=NodePort \
      --set service.nodePort=30080 \
      --wait --timeout 10m
    # The local image tag is reused; restart Pods so repeat runs use the new build
    # and any updated Secret values.
    for role in server worker; do
      kubectl -n "$namespace" rollout restart "deployment/$release-aidash-$role"
      kubectl -n "$namespace" rollout status "deployment/$release-aidash-$role" --timeout=300s
    done
    for _ in {1..30}; do
      if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
        echo "Aidash: http://127.0.0.1:$port"
        echo "Sign in with AIDASH_API_TOKEN from .env, or local-development-token when .env is absent."
        exit 0
      fi
      sleep 1
    done
    echo "Aidash health check failed at http://127.0.0.1:$port/health" >&2
    exit 1
    ;;
  status)
    require_tools kind kubectl
    if ! cluster_exists || [[ ! -s "$KUBECONFIG" ]]; then
      echo "Aidash local kind cluster is not running"
      exit 1
    fi
    kubectl -n "$namespace" get pods,services
    echo "Local access: http://127.0.0.1:$(cat "$state/port")"
    ;;
  down)
    require_tools kind
    if [[ -s "$KUBECONFIG" ]] && cluster_exists; then
      kind delete cluster --name "$cluster" --kubeconfig "$KUBECONFIG"
    fi
    rm -f "$KUBECONFIG"
    rm -f "$state/port" "$state/kind.yaml" "$state/forward.pid" "$state/forward.port" "$state/forward.log"
    ;;
  *)
    echo "Usage: $0 up|status|down" >&2
    exit 2
    ;;
esac
