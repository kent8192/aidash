FROM postgres:17-bookworm

ARG PG_JSONSCHEMA_VERSION=0.3.4

RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
      amd64) asset="pg_jsonschema-v${PG_JSONSCHEMA_VERSION}-pg17-amd64-linux-gnu.deb"; \
             checksum="4c339c178a7396b4076fb1eb5ffe3430bafd3d5f92fb6c917ad49860237db6c9" ;; \
      arm64) asset="pg_jsonschema-v${PG_JSONSCHEMA_VERSION}-pg17-arm64-linux-gnu.deb"; \
             checksum="312f3b6b7b0e9f4d55c1ecd7b16c774da62482869e64745594b8c0f8af753414" ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    curl --fail --location --silent --show-error \
      "https://github.com/supabase/pg_jsonschema/releases/download/v${PG_JSONSCHEMA_VERSION}/${asset}" \
      --output /tmp/pg_jsonschema.deb; \
    echo "${checksum}  /tmp/pg_jsonschema.deb" | sha256sum --check --status; \
    dpkg --install /tmp/pg_jsonschema.deb; \
    rm /tmp/pg_jsonschema.deb; \
    apt-get purge -y --auto-remove curl; \
    rm -rf /var/lib/apt/lists/*

COPY scripts/init-db.sql /docker-entrypoint-initdb.d/01-databases.sql
