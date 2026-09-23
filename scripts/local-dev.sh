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
    export AIDASH_ENDPOINT=${AIDASH_ENDPOINT:-http://127.0.0.1:8080}
    export AIDASH_LISTEN=${AIDASH_LISTEN:-127.0.0.1:8080}
    export AIDASH_API_TOKEN=${AIDASH_API_TOKEN:-local-development-token}
    export AIDASH_BACKEND=${AIDASH_BACKEND:-http://127.0.0.1:8080}
    export AIDASH_FRONTEND_PORT=${AIDASH_FRONTEND_PORT:-5173}

    command -v docker >/dev/null || { echo "Missing required tool: docker" >&2; exit 1; }
    command -v cargo >/dev/null || { echo "Missing required tool: cargo" >&2; exit 1; }
    command -v npm >/dev/null || { echo "Missing required tool: npm" >&2; exit 1; }
    docker compose version >/dev/null

    if [[ ! -x web/node_modules/.bin/vite ]]; then
      npm ci --prefix web
    fi
    docker compose up --detach --wait postgres nats qdrant

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

    cargo run --locked --bin aidash -- serve &
    backend_pid=$!
    npm run dev --prefix web -- --host 127.0.0.1 --port "$AIDASH_FRONTEND_PORT" --strictPort &
    frontend_pid=$!

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
