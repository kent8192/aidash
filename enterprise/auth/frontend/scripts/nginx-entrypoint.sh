#!/bin/sh
set -eu

# The root filesystem may be read-only; only /tmp needs a writable tmpfs.
mkdir -p "$NGINX_ENVSUBST_OUTPUT_DIR"
exec /docker-entrypoint.sh "$@"
