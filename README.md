# Aidash

Aidash 0.1 is a self-hosted federated agent mesh. Register an explicitly selected model and an agent, start a goal in the dashboard, and let agents claim work across independently operated nodes. Workspaces retain tasks, artifacts, messages and an ordered event log. Workers persist their execution state and recover after process termination.

The current implementation provides the federated mesh, scoped authorization and policy-driven local agent generation described in [architecture](docs/architecture.md), [authorization](docs/authorization.md), [generation](docs/generation.md), and [protocol and recovery contracts](docs/protocol.md). Kubernetes orchestration, cross-node atomic transactions, semantic vector memory and full A2A compatibility remain under development.

## Run locally

Prerequisites: Rust 1.96, Node.js 22.18 or later, Docker Compose, and Trunk CLI. The application does not load `.env` automatically.

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

The [authorization API](docs/authorization.md) issues revocable subject tokens for tenant-scoped workspaces, approved Registry discovery, local agent execution and event streams. Workers recheck the root and delegated agents at every durable boundary. The dashboard supports subject tokens for local goals, conversations, human answers and run controls, and shows their tenant identity. Scoped remote federation and dashboard policy-management controls remain under implementation.

The **Registry** screen can register models, tools, skills, clusters and agents. Register a model before an agent. OpenAI-compatible `/chat/completions`, Anthropic `/messages` and OpenRouter `/chat/completions` endpoints are supported; use a base endpoint ending in `/v1`. Specify the provider's actual model ID, context window, modalities and cost metadata. Credentials are resolved only from `AIDASH_SECRET_*` environment variables. Registry records store the environment variable name, never its value. Model selection is explicit; Aidash does not select fallback models or automatically route between models.

For **OpenRouter**, set `AIDASH_SECRET_OPENROUTER` to your OpenRouter API key in the environment of each node's server and worker processes. Select **OpenRouter** in the Registry model form; it fills in `https://openrouter.ai/api/v1` as the editable base endpoint. Enter an explicit model slug from the [OpenRouter model catalog](https://openrouter.ai/models), its context window and cost metadata, and `AIDASH_SECRET_OPENROUTER` as the credential reference. Choose a text model that supports tool calling, which agents require. A model's Registry `config` looks like this (replace the illustrative model ID and context window with the selected model's values):

```json
{
  "provider": "openrouter",
  "model_id": "provider/model-name",
  "endpoint": "https://openrouter.ai/api/v1",
  "credential_env": "AIDASH_SECRET_OPENROUTER",
  "context_window": 128000,
  "modalities": ["text"],
  "cost": {}
}
```

OpenRouter requests use Bearer authentication, `max_tokens` and the shared OpenAI-compatible tool-call and usage parser, following the [OpenRouter API contract](https://openrouter.ai/docs/api/reference/overview). OpenRouter's upstream provider routing follows your OpenRouter account settings; Aidash adds no routing overrides. Inference uses the agent's selected model. History compaction uses Jev separately, as described below. Local tests use protocol fixtures and do not make paid OpenRouter calls.

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

Configure a peer on **both** nodes in Settings. Each peer record contains the other node's identity and endpoint, protocol `0.1`, and the name of a pair-specific `AIDASH_SECRET_*` credential. Each enabled peer must resolve to a different credential; configure the same pair credential at its two endpoints. Peer credentials must contain at least 32 printable ASCII characters and eight distinct characters; use a randomly generated token. Register at least one research agent on each node. Registry discovery exchanges metadata over the federation API; no remote database access is needed. Each workspace retains an authoritative home node.

`aidash server` runs the API, outbox publisher and JetStream consumer. `aidash worker` runs four workers without an HTTP listener. `aidash serve` runs both roles. To exercise recovery, stop a **worker** process while leaving its server, PostgreSQL and NATS running, then restart it with the same configuration. The lease expires after 30 seconds. Task and run IDs remain stable.

## Tools and coordination

Every agent receives these workspace tools: `agent_discover`, `task_create`, `task_delegate`, `artifact_publish`, `workspace_message`, `workspace_observe`, `workspace_wait`, `memory_write`, and `human_request`. The model's final text completes its task and publishes a final artifact. A coordinator must wait for its subtasks and synthesize their artifacts. If a child fails, is blocked, or is cancelled, open its task details and explicitly abandon it with a reason; then answer the parent's human request to resume synthesis. Abandonment is audited and never turns a failed child into a successful result.

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

The Agent tool creates and delegates a child of the invoking task by default, returning its task ID. Native HTTP retrieval is restricted to configured hosts and does not follow redirects. MCP uses the Rust SDK's streamable HTTP transport, including initialization and session lifecycle. No shell or arbitrary code execution tool is enabled by default.

HTTP tools receive an `Idempotency-Key` header. An idempotent MCP tool must specify an argument name that its server actually supports. `read_only` permits safe repetition. `unsafe` allows one attempt; an interrupted or ambiguous effect pauses for reconciliation instead of being invoked again. See the recovery contract before connecting an effectful tool.

## History compaction

Context compaction uses a Rust reimplementation of [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction), with a separate TypeSafe Jev connection. Set `AIDASH_SECRET_JEV` to your TypeSafe API key on each worker (or combined `serve` process). Optional `AIDASH_JEV_ENDPOINT` and `AIDASH_JEV_MODEL` default to `https://api.typesafe.ai/v1/systemone` and `jev-latest`. Jev is contacted only when the model's context budget is exceeded and old tool pairs can be pruned; those requests send a fitted history view to TypeSafe and use that account's credits.

Jev decides whether to keep each old tool call and its full result. Aidash keeps both, keeps the call with a shortened result, or removes the pair. It preserves the first and latest six history events, all human/non-tool events, current task/workspace/memory, and any legacy summary. It creates no new summaries and never silently falls back to summarization. Missing credentials, invalid decisions or insufficient reduction fail the step without changing its saved context. See the [compaction contract](docs/protocol.md#context-compaction) for input fitting and request limits. Protocol tests use a local Jev fixture; live service access and decision quality are not established by those tests.

## Generated API client

Axum management routes and Rust request/response types define the OpenAPI contract through `utoipa` and `utoipa-axum`. The public `/api/openapi.json` endpoint and `aidash openapi` command export the same document; the command needs no database, broker, or credentials. All other `/api` routes require a bearer token.

After changing an API route or type, run `scripts/generate-api.sh`. It exports `openapi/aidash.json` and runs the pinned Orval generator to update `web/src/generated/`. These outputs are ignored by Git, lint and coverage. The dashboard predev and prebuild scripts regenerate them automatically. Dashboard requests and types use these generated files; `transport.ts` supplies authentication/error handling, while `api.ts` reads and reconnects the SSE stream. The Orval transformer exposes unbounded `text/event-stream` responses as `Response`, so the browser can read frames without buffering the entire stream. Do not edit generated files by hand. CI generates them from a clean checkout before building and testing the UI.

## Verification

```sh
trunk fmt --all
trunk check --all --no-fix
scripts/check.sh
```

Trunk owns formatting and linting: rustfmt, Clippy, Prettier, ESLint, Ruff and Taplo. The Rust edition and linter versions are pinned. React Compiler is not enabled, so its incompatible-library diagnostic is disabled for the intentionally mutable TanStack Table/Virtual interfaces; the hook correctness and accessibility rules remain enabled.

`check.sh` runs Rust unit and PostgreSQL integration tests, builds the dashboard, and executes the two-node acceptance scenario starting from a real Chromium dashboard. It also verifies remote human controls, all four tool transports, and the browser scenarios. The scenario starts real Aidash processes, PostgreSQL and NATS with deterministic OpenAI/Anthropic protocol fixtures. The same checks and Trunk lint run in GitHub Actions. Both nodes and workers start with NATS unavailable; the scenario verifies queued events drain after the broker connection is restored. It kills Node B's worker after an external effect but before its result is persisted, restarts the worker, and checks the same run completes with no duplicate effect. Reports are written to `.ignore/acceptance/report.json`.

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

## Database migrations

Schema changes live in the SeaORM migration crate. Create the next migration with `sea-orm-cli migrate generate <name>`, then implement its `up` and `down` methods. Run it with `sea-orm-cli migrate up`; application startup uses the same migrator. Existing SQLx-era databases are adopted only after their recorded checksums are verified. Adopted migrations cannot be rolled back through SeaORM.

## CI and coverage

CI runs Trunk, Rust unit/integration tests, and the PostgreSQL/NATS/Chromium acceptance suite in separate jobs. `CI Success` requires every job to succeed, including the Codecov upload. Use that check for branch protection. Rust coverage uses `cargo llvm-cov` with real PostgreSQL tests and uploads an explicit LCOV file through Codecov OIDC. Codecov measures `src/`; tests, migration plumbing and generated API files are excluded. Browser tests establish dashboard behavior and are not included in the Rust coverage percentage.

Run `scripts/test-rust.sh --coverage` to produce `coverage/rust.lcov` locally (requires `cargo-llvm-cov` 0.8.7 and `llvm-tools-preview`). `scripts/check.sh` runs the full local suite. Cargo and npm lockfiles remain tracked for reproducible dependency resolution.

Package installation overlays the supplied node-local configuration onto the entity configuration, validates it, and publishes the effective immutable Registry version atomically with the installation record. Changing an installed configuration requires a new version.

Third-party attribution for the adapted context compaction code is in [LICENSE](LICENSE).
