# Aidash

Aidash 0.1 is a self-hosted federated agent mesh. Register an explicitly selected model and an agent, start a goal in the dashboard, and let agents claim work across independently operated nodes. Workspaces retain tasks, artifacts, messages and an ordered event log. Workers persist their execution state and recover after process termination.

The release scope is defined by the [v0.1.0 functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24). The current implementation covers the original mesh baseline. Kubernetes/k3s orchestration, automatic agent generation, complex RBAC/ABAC, complete distributed transactions, semantic memory/vector DB and full A2A compatibility are now [required additions](docs/expanded-requirements.md); their implementation and acceptance remain pending. See [architecture](docs/architecture.md), [requirement mapping](docs/requirements.md), and [protocol and recovery contracts](docs/protocol.md).

![Aidash dashboard](docs/images/overview.png)

## Run locally

Prerequisites: Rust 1.96, Node.js 22, Docker Compose, and Trunk CLI. The application does not load `.env` automatically.

```sh
docker compose up -d --wait
npm ci --prefix web
npm run build --prefix web
cargo build --locked
cp .env.example .env
set -a
source .env
set +a
cargo run --locked -- serve
```

Open <http://127.0.0.1:8080> and enter the token from `AIDASH_API_TOKEN`. The example credentials and localhost bindings are for local development. Configure unique credentials and an HTTPS endpoint for a deployed node.

The **Registry** screen can register models, tools, skills, clusters and agents. Register a model before an agent. Both OpenAI-compatible `/chat/completions` and Anthropic `/messages` endpoints are supported; use a base endpoint ending in `/v1`. Specify the provider's actual model ID, context window, modalities and cost metadata. Credentials are resolved only from `AIDASH_SECRET_*` environment variables. Registry records store the environment variable name, never its value. Model selection is explicit; there is no fallback or automatic model routing.

For a development frontend with hot reload:

```sh
npm run dev --prefix web
```

The Vite server proxies API requests to port 8080. Override `AIDASH_BACKEND` when using a different local node.

## Two nodes

Compose creates `aidash_a`, `aidash_b`, and `aidash_test`. Node B needs its own database, identity and port:

```sh
DATABASE_URL=postgres://aidash:aidash-local@127.0.0.1:54370/aidash_b \
AIDASH_NODE_ID=aidash://node-b \
AIDASH_ENDPOINT=http://127.0.0.1:8081 \
AIDASH_LISTEN=127.0.0.1:8081 \
cargo run --locked -- serve
```

Configure a peer on **both** nodes in Settings. Each peer record contains the other node's identity and endpoint, protocol `0.1`, and the name of a shared `AIDASH_SECRET_*` credential. Register at least one research agent on each node. Registry discovery exchanges metadata over the federation API; no remote database access is needed. Each workspace retains an authoritative home node.

`aidash server` runs the API, outbox publisher and JetStream consumer. `aidash worker` runs four workers without an HTTP listener. `aidash serve` runs both roles. To exercise recovery, stop a **worker** process while leaving its server, PostgreSQL and NATS running, then restart it with the same configuration. The lease expires after 30 seconds. Task and run IDs remain stable.

## Tools and coordination

Every agent receives these workspace tools: `agent_discover`, `task_create`, `task_delegate`, `artifact_publish`, `workspace_message`, `workspace_observe`, `workspace_wait`, `memory_write`, and `human_request`. The model's final text completes its task and publishes a final artifact. A coordinator must wait for its subtasks and synthesize their artifacts.

Additional tools are versioned Registry entities. Their JSON Schema validates arguments. Agents reference exact tool versions; provider-safe aliases `plugin_0`, `plugin_1`, etc. follow the order of those references. Supported configurations:

```json
{ "transport": "native", "operation": "echo" }
```

```json
{
  "transport": "native",
  "operation": "http_get",
  "allowed_hosts": ["docs.rs", "github.com"]
}
```

```json
{
  "transport": "http",
  "endpoint": "https://tools.example.com/research",
  "credential_env": "AIDASH_SECRET_TOOLS",
  "replay": "idempotent"
}
```

```json
{
  "transport": "mcp",
  "endpoint": "https://tools.example.com/mcp",
  "credential_env": "AIDASH_SECRET_MCP",
  "tool_name": "search",
  "replay": "read_only",
  "idempotency_argument": null
}
```

```json
{
  "transport": "agent",
  "node_id": "aidash://node-b",
  "agent": { "id": "researcher", "version": "1.0.0" }
}
```

The Agent tool creates and delegates a subtask, returning its task ID. Native HTTP retrieval is restricted to configured hosts and does not follow redirects. MCP uses the Rust SDK's streamable HTTP transport, including initialization and session lifecycle. No shell or arbitrary code execution tool is enabled by default.

HTTP tools receive an `Idempotency-Key` header. An idempotent MCP tool must specify an argument name that its server actually supports. `read_only` permits safe repetition. `unsafe` allows one attempt; an interrupted or ambiguous effect pauses for reconciliation instead of being invoked again. See the recovery contract before connecting an effectful tool.

## Verification

```sh
trunk fmt --all
trunk check --all --no-fix
scripts/check.sh
```

Trunk owns formatting and linting: rustfmt, Clippy, Prettier, ESLint, Ruff and Taplo. The Rust edition and linter versions are pinned. React Compiler is not enabled, so its incompatible-library diagnostic is disabled for the intentionally mutable TanStack Table/Virtual interfaces; the hook correctness and accessibility rules remain enabled.

`check.sh` runs Rust unit and PostgreSQL integration tests, builds the dashboard, and executes the two-node acceptance scenario starting from a real Chromium dashboard. It also verifies remote human controls, all four tool transports, and three browser scenarios. The scenario starts real Aidash processes, PostgreSQL and NATS with deterministic OpenAI/Anthropic protocol fixtures. The same checks and Trunk lint run in GitHub Actions. It kills Node B's worker after an external effect but before its result is persisted, restarts the worker, and checks the same run completes with no duplicate effect. Reports are written to `.ignore/acceptance/report.json`.

For browser tests, keep that completed fixture environment running in one terminal:

```sh
python3 scripts/golden_path.py --binary "$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/aidash")')" --keep
```

In another terminal:

```sh
(cd web && npm exec -- playwright install chromium)
npm test --prefix web
```

Protocol fixtures verify transport, coordination and recovery. They do not establish live model answer quality or provider-account availability. No commercial model calls are made by these tests.
