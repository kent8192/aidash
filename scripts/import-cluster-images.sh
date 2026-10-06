#!/usr/bin/env bash
# Import into the caller's owned k3d node without Docker exec stdin streaming.
set -euo pipefail
node="${1:?Usage: scripts/import-cluster-images.sh NODE IMAGE [IMAGE ...]}"
shift
if [[ "$#" == 0 ]]; then exit 2; fi
archive_dir=$(mktemp -d /tmp/aidash-cluster-images.XXXXXX)
cleanup() { rm -rf "$archive_dir"; }
trap cleanup EXIT
archive="$archive_dir/images.tar"
destination="/tmp/$(basename "$archive_dir").tar"
docker save --output "$archive" "$@"
docker cp "$archive" "$node:$destination"
docker exec "$node" ctr --namespace k8s.io images import --all-platforms "$destination"
for image in "$@"; do
  docker exec "$node" crictl \
    --runtime-endpoint unix:///run/k3s/containerd/containerd.sock \
    inspecti "$image" > /dev/null
done
docker exec "$node" rm -f "$destination"
