FROM postgres:17-bookworm AS memory-build

# Sources are pinned and checked on both supported architectures.
ARG PGVECTOR_VERSION=0.8.7
ARG PGROONGA_VERSION=4.0.9
ARG GROONGA_VERSION=16.1.2
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends build-essential ca-certificates curl pkg-config postgresql-server-dev-17 liblz4-dev libzstd-dev zlib1g-dev meson ninja-build ruby; \
    curl -fsSL --retry 5 "https://codeload.github.com/pgvector/pgvector/tar.gz/refs/tags/v${PGVECTOR_VERSION}" -o /tmp/pgvector.tar.gz; \
    echo "cac0b10c360f05b2d521200105ba3697e773d4cd3731f5a915a7e37ebe0bea85  /tmp/pgvector.tar.gz" | sha256sum -c -; \
    curl -fsSL --retry 5 "https://github.com/groonga/groonga/releases/download/v${GROONGA_VERSION}/groonga-${GROONGA_VERSION}.tar.gz" -o /tmp/groonga.tar.gz; \
    echo "33133af34e8770522c5e80096eb6d94ec88c530201aec81ad430ad7073f9320a  /tmp/groonga.tar.gz" | sha256sum -c -; \
    curl -fsSL --retry 5 "https://github.com/pgroonga/pgroonga/releases/download/${PGROONGA_VERSION}/pgroonga-${PGROONGA_VERSION}.tar.gz" -o /tmp/pgroonga.tar.gz; \
    echo "7d9fd0d8380ef0e807683c30ea25934e0bf5cfdc9553ad17a5907b620e0cbf72  /tmp/pgroonga.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/groonga.tar.gz -C /tmp; \
    cd "/tmp/groonga-${GROONGA_VERSION}"; \
    ./configure --prefix=/usr --disable-document --without-mecab --disable-static; \
    make -j2; make install; \
    tar -xzf /tmp/pgvector.tar.gz -C /tmp; \
    make -C "/tmp/pgvector-${PGVECTOR_VERSION}" -j2 OPTFLAGS=""; \
    make -C "/tmp/pgvector-${PGVECTOR_VERSION}" install; \
    tar -xzf /tmp/pgroonga.tar.gz -C /tmp; \
    meson setup /tmp/pgroonga-build "/tmp/pgroonga-${PGROONGA_VERSION}" --buildtype=release; \
    meson compile -C /tmp/pgroonga-build -j2; \
    meson install -C /tmp/pgroonga-build

FROM postgres:17-bookworm AS extensions

COPY --from=memory-build /usr/lib/postgresql/17/lib/ /usr/lib/postgresql/17/lib/
COPY --from=memory-build /usr/share/postgresql/17/extension/ /usr/share/postgresql/17/extension/
COPY --from=memory-build /usr/lib/libgroonga* /usr/lib/
RUN apt-get update && apt-get install -y --no-install-recommends liblz4-1 libzstd1 && rm -rf /var/lib/apt/lists/* && ldconfig

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
    curl --fail --location --silent --show-error --retry 5 --retry-delay 2 \
      "https://github.com/supabase/pg_jsonschema/releases/download/v${PG_JSONSCHEMA_VERSION}/${asset}" \
      --output /tmp/pg_jsonschema.deb; \
    echo "${checksum}  /tmp/pg_jsonschema.deb" | sha256sum --check --status; \
    dpkg --install /tmp/pg_jsonschema.deb; \
    rm /tmp/pg_jsonschema.deb; \
    apt-get purge -y --auto-remove curl; \
    rm -rf /var/lib/apt/lists/*

# The supported single-primary profile uses Groonga WAL recovery. This is not
# a claim of PGroonga streaming-replication/HA acceptance.
RUN printf "\nshared_preload_libraries = 'pgroonga_crash_safer'\npgroonga.enable_crash_safe = on\nmax_worker_processes = 96\n" >> /usr/share/postgresql/postgresql.conf.sample

# Integration fixtures create their own databases and execute the migration graph.
FROM extensions AS test

FROM extensions AS runtime
COPY scripts/init-db.sql /docker-entrypoint-initdb.d/01-databases.sql
