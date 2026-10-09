#!/usr/bin/env bash
# Verify the rendered admission pair in a disposable network-isolated container.
set -euo pipefail
cd "$(dirname "$0")/.."
image="${AIDASH_EDGE_TEST_IMAGE:-aidash155-edge-admission:local}"
docker build -f infra/gcp/helm/environment/edge.Dockerfile -t "$image" infra/gcp/helm/environment
docker run --rm --network none --label purpose=aidash-155-edge-check \
  -v "$PWD/infra/gcp/helm/environment/files:/source:ro" "$image" bash -euc '
sed "s/AIDASH_BACKEND/127.0.0.1/g" /source/admission.conf > /etc/nginx/conf.d/aidash.conf
nginx -t
nginx
trap "nginx -s quit" EXIT
request() {
  local port=$1 method=$2 path=$3
  exec 3<>/dev/tcp/127.0.0.1/$port
  printf "%s %s HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" "$method" "$path" >&3
  cat <&3
  exec 3<&- 3>&-
}
closed=$(request 8089 GET /activity)
[[ "$closed" == *"\"closed\":true"* ]]
public=$(request 8088 GET /api/state)
[[ "$public" == *"503 Service Temporarily Unavailable"* ]]
request 8089 POST /admission/open >/dev/null
opened=$(request 8089 GET /activity)
[[ "$opened" == *"\"closed\":false"* ]]
request 8089 POST /admission/close >/dev/null
closed=$(request 8089 GET /activity)
[[ "$closed" == *"\"closed\":true"* ]]
printf "%s\n" "Chart Nginx/Lua: starts closed; public admission blocked; private open/close verified"
'
