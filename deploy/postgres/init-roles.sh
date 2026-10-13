#!/usr/bin/env bash
# Provision Aidash's database roles on a new data directory (#262).
#
# Migrations log in as aidash_owner, which owns every table. Servers and workers
# log in as aidash_server, which holds only DML, so row-level security binds it.
# The bootstrap superuser only creates the roles and the extensions, which
# require a superuser. Compose and local Kubernetes pass both passwords; a
# deployment that provisions roles itself, such as a GKE Environment, passes
# neither, and this script does nothing.
set -euo pipefail

if [[ -z "${AIDASH_OWNER_PASSWORD:-}" && -z "${AIDASH_SERVER_PASSWORD:-}" ]]; then
  exit 0
fi
: "${AIDASH_OWNER_PASSWORD:?AIDASH_SERVER_PASSWORD needs AIDASH_OWNER_PASSWORD}"
: "${AIDASH_SERVER_PASSWORD:?AIDASH_OWNER_PASSWORD needs AIDASH_SERVER_PASSWORD}"

run() {
  psql --no-psqlrc --quiet -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" "$@"
}

run --dbname postgres \
  -v owner_password="$AIDASH_OWNER_PASSWORD" \
  -v server_password="$AIDASH_SERVER_PASSWORD" <<'SQL'
CREATE ROLE aidash_owner LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD :'owner_password';
CREATE ROLE aidash_server LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD :'server_password';
SQL

# 01-databases.sql created the other local databases before this script.
for database in "$POSTGRES_DB" aidash_b aidash_test; do
  run --dbname postgres -c "ALTER DATABASE \"$database\" OWNER TO aidash_owner"
  # Default privileges apply only to objects the owner creates afterwards.
  run --dbname "$database" <<'SQL'
CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public;
CREATE EXTENSION IF NOT EXISTS vector;
ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_server;
ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT USAGE, SELECT ON SEQUENCES TO aidash_server;
SQL
  # PGroonga's crash-safe worker starts asynchronously in each database. Only
  # its documented preparing state is retried, each time on a fresh backend.
  for attempt in $(seq 1 300); do
    if output=$(run --dbname "$database" \
      -c "CREATE EXTENSION IF NOT EXISTS pgroonga" \
      -c "SELECT pgroonga_command('io_flush')" 2>&1 >/dev/null); then
      break
    fi
    if [[ "$output" != *"pgroonga_crash_safer is preparing"* || "$attempt" == 300 ]]; then
      echo "$output" >&2
      exit 1
    fi
    sleep 0.1
  done
done
