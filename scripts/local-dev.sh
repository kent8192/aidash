#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

case "${1:-up}" in
  up)
    if [[ -f .env ]]; then
      set -a
      # Local .env follows the shell format used in the README.
      # shellcheck disable=SC1091 # Optional, user-provided file.
      source .env
      set +a
    fi

    export AIDASH_POSTGRES_PORT=${AIDASH_POSTGRES_PORT:-54370}
    export AIDASH_NATS_PORT=${AIDASH_NATS_PORT:-42270}
    export AIDASH_QDRANT_PORT=${AIDASH_QDRANT_PORT:-63370}
    export DATABASE_URL=${DATABASE_URL:-postgres://aidash:aidash-local@127.0.0.1:${AIDASH_POSTGRES_PORT}/aidash_a}
    export NATS_URL=${NATS_URL:-nats://127.0.0.1:${AIDASH_NATS_PORT}}
    export AIDASH_NODE_ID=${AIDASH_NODE_ID:-aidash://node-a}
    export AIDASH_LISTEN=${AIDASH_LISTEN:-127.0.0.1:18080}
    backend_health_host=${AIDASH_LISTEN%:*}
    backend_listen_port=${AIDASH_LISTEN##*:}
    case "$backend_health_host" in
      0.0.0.0|\*|localhost|'') backend_health_host=127.0.0.1 ;;
    esac
    backend_url="http://$backend_health_host:$backend_listen_port"
    export AIDASH_ENDPOINT=${AIDASH_ENDPOINT:-$backend_url}
    export AIDASH_API_TOKEN=${AIDASH_API_TOKEN:-local-development-token}
    export AIDASH_BACKEND=${AIDASH_BACKEND:-$backend_url}
    export AIDASH_FRONTEND_PORT=${AIDASH_FRONTEND_PORT:-5173}
    export AIDASH_SECRET_TEST_QDRANT=${AIDASH_SECRET_TEST_QDRANT:-local-semantic-vector-fixture-key-0123456789}
    backend_health_url="http://$backend_health_host:$backend_listen_port/health"

    command -v docker >/dev/null || { echo "Missing required tool: docker" >&2; exit 1; }
    command -v cargo >/dev/null || { echo "Missing required tool: cargo" >&2; exit 1; }
    command -v npm >/dev/null || { echo "Missing required tool: npm" >&2; exit 1; }
    command -v node >/dev/null || { echo "Missing required tool: node" >&2; exit 1; }
    command -v curl >/dev/null || { echo "Missing required tool: curl" >&2; exit 1; }
    docker compose version >/dev/null

    port_is_in_use() {
      local host="$1" port="$2"
      (exec 3<>"/dev/tcp/$host/$port") >/dev/null 2>&1
    }
    if port_is_in_use "$backend_health_host" "$backend_listen_port"; then
      echo "Backend address $AIDASH_LISTEN is already in use; choose another AIDASH_LISTEN and AIDASH_BACKEND." >&2
      exit 1
    fi
    if port_is_in_use 127.0.0.1 "$AIDASH_FRONTEND_PORT"; then
      echo "Frontend address 127.0.0.1:$AIDASH_FRONTEND_PORT is already in use; choose another AIDASH_FRONTEND_PORT." >&2
      exit 1
    fi

    if ! npm ls --prefix web --depth=0 >/dev/null 2>&1; then
      echo "Installing web dependencies from web/package-lock.json."
      npm ci --prefix web
    fi
    docker compose up --build --detach --wait postgres nats qdrant

    # Compose initialization scripts only run for a new data volume. Ensure
    # retained local databases also have the extension required by migrations.
    for database in aidash_a aidash_b aidash_test; do
      docker compose exec -T postgres psql -v ON_ERROR_STOP=1 -U aidash -d "$database" \
        -c 'CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public'
      extension=$(docker compose exec -T postgres psql -U aidash -d "$database" -Atc \
        "SELECT n.nspname || ':' || e.extversion FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='pg_jsonschema'")
      if [[ "$extension" != "public:0.3.4" ]]; then
        echo "Expected pg_jsonschema 0.3.4 in public for database $database; found ${extension:-not installed}." >&2
        exit 1
      fi
    done

    backend_pid=
    frontend_pid=
    stop_processes() {
      trap - EXIT INT TERM
      for pid in "${frontend_pid:-}" "${backend_pid:-}"; do
        if [[ -n "$pid" ]]; then
          kill -TERM "$pid" 2>/dev/null || true
        fi
      done
      for pid in "${frontend_pid:-}" "${backend_pid:-}"; do
        if [[ -n "$pid" ]]; then
          wait "$pid" 2>/dev/null || true
        fi
      done
    }
    trap stop_processes EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    node scripts/run-process-tree.mjs cargo run --locked --bin aidash -- serve &
    backend_pid=$!
    node scripts/run-process-tree.mjs npm run dev --prefix web -- --port "$AIDASH_FRONTEND_PORT" --strictPort &
    frontend_pid=$!

    wait_for_url() {
      local label="$1" url="$2" pid="$3" probe_url="${4:-}"
      for _ in {1..120}; do
        if ! jobs -pr | grep -Fxq "$pid"; then
          echo "$label stopped before becoming ready." >&2
          return 1
        fi
        if curl -fsS --max-time 1 "$url" >/dev/null 2>&1; then
          if [[ -z "$probe_url" ]] || curl -fsS --max-time 1 "$probe_url" >/dev/null 2>&1; then
            if jobs -pr | grep -Fxq "$pid"; then
              return 0
            fi
            echo "$label process stopped before becoming ready." >&2
            return 1
          fi
        fi
        sleep 1
      done
      echo "$label did not become ready at $url." >&2
      return 1
    }

    wait_for_url "Aidash backend" "$backend_health_url" "$backend_pid"
    wait_for_url "Vite frontend" "http://127.0.0.1:$AIDASH_FRONTEND_PORT/" "$frontend_pid" \
      "http://127.0.0.1:$AIDASH_FRONTEND_PORT/src/main.tsx"

    echo "Frontend: http://127.0.0.1:$AIDASH_FRONTEND_PORT"
    echo "Backend:  $AIDASH_BACKEND"
    while true; do
      running="$(jobs -pr)"
      if ! printf '%s\n' "$running" | grep -Fxq "$backend_pid"; then
        echo "Aidash backend stopped; stopping the frontend." >&2
        exit 1
      fi
      if ! printf '%s\n' "$running" | grep -Fxq "$frontend_pid"; then
        echo "Vite frontend stopped; stopping the backend." >&2
        exit 1
      fi
      sleep 1
    done
    ;;
  down)
    docker compose down
    ;;
  *)
    echo "Usage: $0 up|down" >&2
    exit 2
    ;;
esac
