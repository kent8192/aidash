#!/bin/sh
set -eu
# Infrastructure-only role/schema/default-privilege DDL; the owning service
# binaries, not this PostgreSQL entrypoint, apply the Reinhardt migrations.
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  -f /bootstrap/auth.sql -f /bootstrap/marketplace.sql
AUTH_OWNER_PASSWORD=$(cat /run/secrets/auth-owner-password)
AUTH_APP_PASSWORD=$(cat /run/secrets/auth-app-password)
MARKETPLACE_OWNER_PASSWORD=$(cat /run/secrets/marketplace-owner-password)
MARKETPLACE_APP_PASSWORD=$(cat /run/secrets/marketplace-app-password)
export AUTH_OWNER_PASSWORD AUTH_APP_PASSWORD MARKETPLACE_OWNER_PASSWORD MARKETPLACE_APP_PASSWORD
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<'SQL'
\getenv auth_owner AUTH_OWNER_PASSWORD
\getenv auth_app AUTH_APP_PASSWORD
\getenv marketplace_owner MARKETPLACE_OWNER_PASSWORD
\getenv marketplace_app MARKETPLACE_APP_PASSWORD
ALTER ROLE aidash_auth_owner LOGIN PASSWORD :'auth_owner';
ALTER ROLE aidash_auth_app LOGIN PASSWORD :'auth_app';
ALTER ROLE marketplace_owner LOGIN PASSWORD :'marketplace_owner';
ALTER ROLE marketplace_app LOGIN PASSWORD :'marketplace_app';
REVOKE ALL ON SCHEMA public FROM PUBLIC;
SQL
