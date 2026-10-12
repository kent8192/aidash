#!/usr/bin/env bash
# Verify the rendered admission pair in a disposable network-isolated container.
set -euo pipefail
cd "$(dirname "$0")/.."
image="${AIDASH_EDGE_TEST_IMAGE:-aidash155-edge-admission:local}"
# The edge image installs from an amd64-only archive snapshot, like its release build.
docker build --platform linux/amd64 -f infra/gcp/helm/environment/edge.Dockerfile -t "$image" infra/gcp/helm/environment
docker run --rm --platform linux/amd64 --network none --label purpose=aidash-155-edge-check \
  -v "$PWD/infra/gcp/helm/environment/files:/source:ro" "$image" bash -euc '
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
