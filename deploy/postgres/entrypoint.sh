#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == -* ]]; then
  set -- postgres "$@"
fi
if [[ "${1:-}" == postgres ]]; then
  # The supported single-primary profile uses Groonga WAL recovery on both new
  # and retained data directories. This does not establish streaming/HA support.
  # An explicit worker count overrides the default; recovery settings are required.
  set -- postgres -c max_worker_processes=96 "${@:2}" \
    -c shared_preload_libraries=pgroonga_crash_safer \
    -c pgroonga.enable_crash_safe=on
fi
exec /usr/local/bin/docker-entrypoint.sh "$@"
