#!/usr/bin/env bash
# Verify the rendered admission pair in a disposable network-isolated container.
set -euo pipefail
cd "$(dirname "$0")/../.."
image="${AIDASH_EDGE_TEST_IMAGE:-aidash155-edge-admission:local}"
# The edge image installs from an amd64-only archive snapshot, like its release build.
docker build --platform linux/amd64 -f enterprise/infra/gcp/helm/environment/edge.Dockerfile -t "$image" enterprise/infra/gcp/helm/environment
docker run --rm --platform linux/amd64 --network none --label purpose=aidash-155-edge-check \
  -v "$PWD/enterprise/infra/gcp/helm/environment/files:/source:ro" "$image" bash -euc '
sed "s/AIDASH_BACKEND/127.0.0.1/g" /source/admission.conf > /etc/nginx/conf.d/aidash.conf
# Stand-in application: every request succeeds.
printf "%s\n" "server { listen 127.0.0.1:8080; access_log off; location / { return 200 \"{}\"; } }" > /etc/nginx/conf.d/backend.conf
# The Pod mounts an emptyDir here (world-writable, outliving container restarts).
mkdir -p /var/lib/aidash-activity
chmod 0777 /var/lib/aidash-activity
nginx -t
request() {
  local port=$1 method=$2 path=$3
  exec 3<>/dev/tcp/127.0.0.1/$port
  printf "%s %s HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n" "$method" "$path" >&3
  cat <&3
  exec 3<&- 3>&-
}
# A (re)started edge restores the controller-owned desired state: only a file
# whose trimmed content is exactly "open" admits; missing or anything else is closed.
start() {
  rm -rf /etc/aidash-admission
  if [[ "$1" != absent ]]; then mkdir /etc/aidash-admission; printf "$1" > /etc/aidash-admission/state; fi
  nginx
  for _ in $(seq 50); do (exec 3<>/dev/tcp/127.0.0.1/8089) 2>/dev/null && return; sleep 0.1; done
  return 1
}
stop() {
  nginx -s stop
  while [[ -e /run/nginx.pid ]]; do sleep 0.1; done
}
for state in absent closed "open\\n" " open " opened OPEN ""; do
  start "$state"
  if [[ "$state" == "open\\n" || "$state" == " open " ]]; then
    [[ "$(request 8089 GET /activity)" == *"\"closed\":false"* ]]
    [[ "$(request 8088 GET /api/state)" == *"200 OK"* ]]
  else
    [[ "$(request 8089 GET /activity)" == *"\"closed\":true"* ]]
    [[ "$(request 8088 GET /api/state)" == *"503 Service Temporarily Unavailable"* ]]
  fi
  stop
done
start absent
trap "nginx -s quit" EXIT
closed=$(request 8089 GET /activity)
[[ "$closed" == *"\"closed\":true"* ]]
public=$(request 8088 GET /api/state)
[[ "$public" == *"503 Service Temporarily Unavailable"* ]]
# A Pod-network peer (here a non-loopback source) may read activity but never
# toggle admission; only the controller exec on loopback can.
peer() { curl -s -o /dev/null -w "%{http_code}" --interface 127.0.0.2 "$@"; }
[[ "$(peer http://127.0.0.1:8089/activity)" == 200 ]]
[[ "$(peer -X POST http://127.0.0.1:8089/admission/open)" == 403 ]]
[[ "$(request 8089 GET /activity)" == *"\"closed\":true"* ]]
request 8089 POST /admission/open >/dev/null
opened=$(request 8089 GET /activity)
[[ "$opened" == *"\"closed\":false"* ]]
[[ "$opened" == *"\"last_active\":0"* ]]
# Session polling stays idle; the dashboard interaction heartbeat renews activity.
request 8088 GET /auth/session >/dev/null
[[ "$(request 8089 GET /activity)" == *"\"last_active\":0"* ]]
request 8088 POST /auth/activity >/dev/null
[[ "$(request 8089 GET /activity)" != *"\"last_active\":0"* ]]
request 8089 POST /admission/close >/dev/null
closed=$(request 8089 GET /activity)
[[ "$closed" == *"\"closed\":true"* ]]
# A restart of the admission container between a meaningful request and the next
# collector sample must not lose the timestamp: the idle interval stays full. The
# stamp is read from the file, so no sample happens between request and restart.
last_active() { request 8089 GET /activity | grep -o "\"last_active\":[^,}]*"; }
same_stamp() { awk -v a="${1#*:}" -v b="$(cat /var/lib/aidash-activity/last_active)" "BEGIN { exit !(a + 0 == b + 0 && b > 0) }"; }
stop
start "open\\n"
same_stamp "$(last_active)"
# The restored stamp is the persisted file, not a coincidence: a fresh emptyDir
# (a replaced Pod) reports never active, and a later request persists again.
stop
rm -f /var/lib/aidash-activity/*
start "open\\n"
[[ "$(last_active)" == "\"last_active\":0" ]]
request 8088 POST /auth/activity >/dev/null
before=$(last_active)
[[ "$before" != "\"last_active\":0" ]]
# Workers are separate processes: a burst of counted requests across them leaves
# the persisted stamp equal to the reported one and never behind an earlier stamp.
[[ "$(grep -c "^processor" /proc/cpuinfo)" -gt 1 ]] || printf "%s\n" "warning: one CPU, the burst runs in one worker" >&2
curl -sf --parallel --parallel-max 32 -X POST $(printf "http://127.0.0.1:8088/auth/activity %.0s" $(seq 200)) >/dev/null
after=$(last_active)
same_stamp "$after"
awk -v a="${after#*:}" -v b="${before#*:}" "BEGIN { exit !(a + 0 >= b + 0) }"
stop
start "open\\n"
same_stamp "$(last_active)"
stop
printf "garbage" > /var/lib/aidash-activity/last_active
start "open\\n"
[[ "$(last_active)" == "\"last_active\":0" ]]
# Requests must not grow a file in the writable layer: logs go to container streams.
[[ -z "$(find /var/log/nginx -type f -size +0c)" ]]
printf "%s\n" "Chart Nginx/Lua: restores only an exact open desired state, otherwise starts closed; public admission blocked; peers cannot toggle admission; private open/close verified; heartbeat counted and kept across an admission restart, polling idle; no file logs"
'

# Exercise the actual digest-pinned Caddy with the rendered random-host config.
# The certificate and hosts are disposable fixtures, and the container has no network.
edge_test_dir=$(mktemp -d)
caddy_container=""
trap 'if [[ -n "$caddy_container" ]]; then docker rm -f "$caddy_container" >/dev/null 2>&1; fi; rm -rf "$edge_test_dir"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
  -subj '/CN=*.preview.aidash.run' -addext 'subjectAltName=DNS:*.preview.aidash.run' \
  -out "$edge_test_dir/tls.crt" -keyout "$edge_test_dir/tls.key" >/dev/null 2>&1
python3 - "$edge_test_dir" <<'PY'
import json
from pathlib import Path
import subprocess
import sys
import yaml

directory = Path(sys.argv[1])
digest = '@sha256:' + 'a' * 64
values = {
    'postgres': {'existingSecret': 'fixture', 'image': 'postgres' + digest},
    'edge': {'hostname': 'a' * 32 + '.preview.aidash.run', 'admissionImage': 'edge' + digest,
             'tlsSecret': 'aidash-preview-tls'},
    'activity': {'existingSecret': 'fixture', 'observerImage': 'observer' + digest,
                 'collectorImage': 'collector' + digest},
}
objects = yaml.safe_load_all(subprocess.check_output(
    ['helm', 'template', 'env', 'enterprise/infra/gcp/helm/environment', '-f', '-'],
    input=json.dumps(values).encode(),
))
caddy = next(value['data']['Caddyfile'] for value in objects
             if value and value['kind'] == 'ConfigMap' and 'Caddyfile' in value['data'])
caddy = caddy.replace('/etc/aidash-preview-tls/', '/fixture/')
# Upstream tries to relax the policy; the edge must overwrite it after proxying.
caddy += '\n:8088 {\n header Referrer-Policy unsafe-url\n respond "backend-ok" 200\n}\n'
(directory / 'Caddyfile').write_text(caddy)
PY
caddy_image=$(python3 -c 'import yaml; print(yaml.safe_load(open("enterprise/infra/gcp/helm/environment/values.yaml"))["edge"]["caddyImage"])')
docker run --rm --network none -v "$edge_test_dir:/fixture:ro" "$caddy_image" \
  caddy validate --config /fixture/Caddyfile --adapter caddyfile
caddy_container=$(docker run -d --network none --label purpose=aidash-random-edge-check \
  -v "$edge_test_dir:/fixture:ro" "$caddy_image" caddy run --config /fixture/Caddyfile --adapter caddyfile)
docker run --rm -i --platform linux/amd64 --network "container:$caddy_container" \
  -v "$edge_test_dir:/fixture:ro" --entrypoint bash "$image" -eu <<'BASH'
known=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.preview.aidash.run
unknown=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.preview.aidash.run
for attempt in $(seq 50); do
  if curl --http1.1 -fsS --cacert /fixture/tls.crt --resolve "$known:443:127.0.0.1" --max-time 1 "https://$known/" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
check() {
  local scheme=$1 host=$2 status=$3 framing=${4:-} port=443 result
  if [[ "$scheme" == http ]]; then port=80; fi
  result=$(curl --http1.1 -sS --cacert /fixture/tls.crt --resolve "$host:$port:127.0.0.1" --max-time 3 -D- -o /dev/null "$scheme://$host/")
  case "$result" in *"HTTP/1.1 $status"*) ;; *) printf "%s\n" "$result"; exit 1;; esac
  case "$result" in *"Referrer-Policy: no-referrer"*|*"referrer-policy: no-referrer"*) ;; *) printf "%s\n" "$result"; exit 1;; esac
  case "$result" in *"Referrer-Policy: unsafe-url"*) exit 1;; esac
  if [[ "$framing" == deny ]]; then
    case "$result" in *"Content-Security-Policy: frame-ancestors 'none'"*|*"content-security-policy: frame-ancestors 'none'"*) ;; *) printf "%s\n" "$result"; exit 1;; esac
    case "$result" in *"X-Frame-Options: DENY"*|*"x-frame-options: DENY"*) ;; *) printf "%s\n" "$result"; exit 1;; esac
  fi
}
check https "$known" 200 deny
check https "$unknown" 404
check http "$unknown" 404
check http "$known" 308
printf "%s\n" "Chart Caddy: static wildcard TLS, exact-host routing, unknown-host HTTPS/HTTP rejected, redirect and backend responses enforce no-referrer, application responses deny framing"
BASH
