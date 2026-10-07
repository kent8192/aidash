# Aidash

Aidash 0.1 is a self-hosted federated agent mesh. Register an explicitly selected model and an agent, start a goal in the dashboard, and let agents claim work across independently operated nodes. Workspaces retain tasks, artifacts, messages and an ordered event log. Workers persist their execution state and recover after process termination.

The current implementation includes a federated mesh, scoped remote Worker activation and Home commands, scoped authorization, policy-driven local agent generation, a recoverable cross-node transaction protocol, persistent semantic memory, Creator and Trust workbenches, and Kubernetes/k3s deployment. See [architecture](docs/architecture.md), [authorization](docs/authorization.md), [generation](docs/generation.md), [distributed transactions](docs/transactions.md), [semantic memory](docs/semantic-memory.md), [orchestration](docs/orchestration.md), and [protocol and recovery contracts](docs/protocol.md). Source and focused test coverage for these paths do not close the full scoped federation (#38), transaction (#40), A2A (#39), or combined release acceptance gates.

## Cargo workspace

All packages use Rust 2024. The virtual workspace defaults to `aidash-server`;
Reinhardt's `manage` CLI provides the standard management operations and Aidash's
registered custom commands, including `serve`, `server`, and `worker`:

```sh
cargo run --locked -p aidash-server --bin manage -- serve
cargo run --locked -p aidash-server --bin manage -- server
cargo run --locked -p aidash-server --bin manage -- worker
cargo run --locked -p aidash-server --bin manage -- --help
```

The `aidash` binary delegates to the same command registry and only preserves
the existing executable name and default `serve` behavior. Existing
`cargo run --locked -- serve`, `server`, and `worker` invocations remain valid.

| Package               | Responsibility                                                           |
| --------------------- | ------------------------------------------------------------------------ |
| `aidash-domain`       | Business models, typed state, and pure invariants.                       |
| `aidash-application`  | Authorized use cases, shared execution contracts, and external ports.    |
| `aidash-harness`      | Agent steps, worker activation, leases, cancellation, and recovery.      |
| `aidash-runtime`      | Background supervision, shutdown, and worker drain.                      |
| `aidash-integrations` | Inference, HTTP/MCP, NATS, Qdrant, and Kubernetes adapters.              |
| `aidash-server`       | Reinhardt HTTP/ORM/settings, app repositories/migrations, and bootstrap. |

`server/src/bootstrap.rs` assembles the concrete adapters for both HTTP and
workers. Apps are ordinary Rust modules. The harness depends on application/domain
contracts; application/domain do not depend on the harness. Domain/application/harness
production dependency closures exclude runtime, integrations, the server, Reinhardt,
Axum, SQLx, SeaORM, and reqwest; CI checks these boundaries against locked Cargo metadata.
The React dashboard remains in
`web/` and uses the existing URL, JSON, authentication, and SSE contracts.

Reinhardt is pinned to development revision
`a068ecbdc03ff01653f80c9c4ab36e15a27f2bd7` in the manifests and lockfile. The initial
project and app scaffolds were generated with the Reinhardt CLI.

## Run locally

### One-command Kubernetes start

With Docker, kind, kubectl, Helm, curl, and cargo-make installed, run:

```sh
cargo make k8s-up
```

This creates a dedicated `aidash-local` kind cluster, builds and imports the
Aidash backend, frontend, and PostgreSQL images (including `pg_jsonschema`),
starts persistent PostgreSQL, JetStream NATS and Qdrant, deploys the server,
worker, and frontend with Helm, and exposes the dashboard at
<http://127.0.0.1:8080>. The frontend Service serves the dashboard and proxies
API, authentication, federation, and health requests to the internal backend Service.

The dashboard requires a configured Google OAuth client. Without one it shows setup guidance; `AIDASH_API_TOKEN` remains available for the API and initial operator grant. Kubernetes startup uses the example Secrets in `deploy/local-k8s/` and does not load `.env`. Add the three `AIDASH_OIDC_*` connection values (`CLIENT_ID`, `CLIENT_SECRET`, and `PUBLIC_ORIGIN`) to the `aidash-local-app` Secret, including `AIDASH_OIDC_PUBLIC_ORIGIN=http://127.0.0.1:8080`, then re-run `cargo make k8s-up` to restart the Pods. Register `http://127.0.0.1:8080/auth/callback` in Google Auth Platform. Google endpoints must be reachable from the backend Pod. Add provider credentials to this Secret as needed:

```sh
kubectl --kubeconfig .ignore/local-k8s/kubeconfig -n aidash-local edit secret aidash-local-app
```

The task keeps a dedicated kubeconfig in `.ignore/local-k8s/` and does not
change the current kubectl context. The local host port is fixed at `8080` in
`deploy/local-k8s/kind.yaml`.

`cargo make k8s-status` shows Pods and Services. `cargo make k8s-down` removes
the dedicated cluster and its persistent local data. Re-running `k8s-up` updates
the images and Helm release while keeping the existing cluster data. Startup
reapplies the example credentials from `deploy/local-k8s/secrets.yaml`; extra
provider keys added to the app Secret are retained.

If a cluster created with the previous Python helper uses a different port or
PostgreSQL password, recreate it with `cargo make k8s-down` followed by
`cargo make k8s-up`. This deletes its local database and other persistent data.

### Docker Compose development

Prerequisites: `cargo-make` and Docker Compose v2.24 or later. Rust 1.96
and Node.js 22 run inside the development images. Start PostgreSQL, NATS,
Qdrant, the backend, and Vite in detached mode:

```sh
cargo make dev
```

Open <http://127.0.0.1:5173> and configure Google OAuth as described below to sign in.
The backend defaults to <http://127.0.0.1:18080>. Compose reads `.env`
directly; set `AIDASH_BACKEND_PORT` or `AIDASH_FRONTEND_PORT` there to change
host ports. Run `docker compose --profile dev logs -f` to follow logs and
`cargo make dev-down` to stop the services while retaining their data volumes.
Re-run `cargo make dev` to rebuild after source changes. The example credentials
and localhost bindings are for local development. Configure unique credentials
and an HTTPS endpoint for a deployed node.

The [authorization API](docs/authorization.md) issues revocable subject tokens for tenant-scoped workspaces, approved Registry discovery, local agent execution and event streams. Workers recheck the root and delegated agents at every durable boundary. The dashboard signs in with Google and requires an explicit mapping to an existing user subject or a separate operator grant. Its selected authority is local to each tab. Existing API Bearer credentials remain available. Operators use **Access policies / アクセス制御** to edit role/attribute policies, simulate decisions, approve registration requests, manage mappings and operator grants, and issue or revoke subject credentials. Scoped remote federation remains under implementation.

### Dashboard OIDC setup

Aidash signs in directly with Google using OAuth 2.0 / OpenID Connect Authorization Code with PKCE. The backend verifies the ID token's signature, issuer, audience, expiry and nonce before creating an HttpOnly session; it never exposes Google tokens or the client secret to the browser. Identity is keyed by issuer and `sub`, never by email.

1. In [Google Auth Platform](https://console.cloud.google.com/auth/overview), configure your application's branding and audience. Add permitted test users while the app is in testing.
2. Create an OAuth client of type **Web application**. Register the exact authorized redirect URI `<origin>/auth/callback`, for example `http://127.0.0.1:5173/auth/callback` for Vite or `http://127.0.0.1:8080/auth/callback` for local Kubernetes.
3. Set `AIDASH_OIDC_CLIENT_ID`, `AIDASH_OIDC_CLIENT_SECRET`, and `AIDASH_OIDC_PUBLIC_ORIGIN` on the backend as shown in `.env.example`, then restart it. `AIDASH_OIDC_ISSUER` defaults to `https://accounts.google.com`. Keep the client secret on the backend only.

The Vite proxy forwards `/auth` to the backend. Production requires an HTTPS dashboard origin, which enables Secure host-only session cookies. Sessions expire after 12 hours or 30 minutes without user activity by default. No Keycloak container, realm, admin API, or status service account is needed. See [Google's OpenID Connect documentation](https://developers.google.com/identity/openid-connect/openid-connect) for provider setup.

Google login does not provide Keycloak's service-account enabled-user lookup or back-channel logout notifications. Google account changes therefore do not immediately revoke existing Aidash sessions or pause admitted work. Aidash continues checking local identity disablement, subject policy, mappings and credentials at execution boundaries; use local mapping/operator-grant revocation to remove access. Aidash logout affects Aidash sessions, not the Google browser session. No offline Google access or refresh token is requested.

Existing Keycloak deployments remain compatible when explicitly configured with their issuer and all three legacy variables: `AIDASH_OIDC_KEYCLOAK_ADMIN_URL`, `AIDASH_OIDC_STATUS_CLIENT_ID`, and `AIDASH_OIDC_STATUS_CLIENT_SECRET`. Their service-account status checks and back-channel logout behavior are unchanged. Switching issuers creates separate external identities: old Keycloak mappings and operator grants are not transferred to Google, even if email addresses match.

Changing issuers requires a coordinated stop/start, including when switching back. Stop every Aidash server and worker that shares the database, update all replicas to the same issuer configuration, and then restart them before allowing sign-in. Do not use a rolling restart: a process still using the old issuer can permanently disable identities and revoke sessions created under the new issuer. See the [issuer-switch procedure](docs/authorization.md#switching-issuers) before migrating.

The first external identity has no authority. After its first sign-in, list verified identity IDs with `GET /api/dashboard/identities` using the existing operator Bearer token, then grant the selected identity operator access with `POST /api/dashboard/identities/{id}/operator-grant` and body `{"enabled":true,"expected_revision":0}`. Applicants can submit an authenticated registration request; operators approve one against an existing enabled user subject in **Access policies**. Approval never creates a subject or grants operator access. For legacy Keycloak deployments, Aidash checks its enabled status at most 15 minutes after the last successful check; a definite disablement revokes browser sessions and pauses work. Once Keycloak access is restored, an operator must explicitly call `POST /api/dashboard/identities/{id}/restore`; previously revoked sessions require a new sign-in. Current-browser and all-device Aidash logout are separate dashboard actions. API Bearer recovery remains available during IdP outages.

The **Registry** screen can register models, tools, skills, clusters, agents, compactors and embedding providers. Register a model before an agent. Inference uses OpenRouter `/chat/completions`. Specify the provider's actual model ID, context window, modalities and cost metadata. Credentials are resolved only from `AIDASH_SECRET_*` environment variables. Registry records store the environment variable name, never its value. Model selection is explicit; Aidash does not select fallback models or automatically route between models.

Registration starts with an editable English `adjective-animal` name and one description field. The server assigns a UUID v7 internally; the registration form does not expose an ID field. The form retains a private request key for unchanged retries, and the server saves its assigned ID atomically with registration. API clients can still supply explicit IDs or send an `Idempotency-Key` UUID header; reusing a key with different input returns a conflict. An ID and version together identify the immutable registration. The form stores name and description in `en` and provides editable discovery languages defaulting to `ja`/`en`. Skills can import existing `SKILL.md` instruction files. Agent creation prioritizes selected Skills, optional additional instructions, and personal PDF/Excel/text references; see [Skills-first agents](docs/features/skills-first-agents.md) for supported formats and execution limits. Clusters select an exact coordinator-agent version, and tools have connection-specific fields plus an argument editor for text, numbers, booleans, groups and lists. Argument editors preserve allowed values, patterns and numeric bounds. Native web tools provide the URL argument automatically. Agent tools use fixed task arguments: required title and description, with optional requirements, dependencies and parent task. No configuration JSON is entered. Authenticated HTTP/MCP tools accept a per-entry credential reference, defaulting to `AIDASH_SECRET_TOOL`; embeddings use `AIDASH_SECRET_EMBEDDING` and compactors use `AIDASH_SECRET_JEV`. These variables must be configured on the server and workers. Existing registrations and their explicit credential references remain unchanged.

For **OpenRouter**, set `AIDASH_SECRET_OPENROUTER` on the server and worker processes. The Registry model form uses OpenRouter: search the live model catalog by name or ID and select a text model with tool calling. Aidash fills the model ID, endpoint, context window, maximum output tokens and indicative per-million-token pricing automatically. Models without a published maximum output limit are not offered. Catalog failures can be retried from the form. No credential reference is entered in the model form; model registration uses `AIDASH_SECRET_OPENROUTER`. Existing registry entries retain their explicit credential references.

The model's advertised maximum output is saved in its immutable Registry version and used for each inference request, context-fit check and generation reservation. OpenRouter requests use Bearer authentication and `max_tokens`. Every inference request enforces `provider.zdr = true`; a model without an available ZDR endpoint fails instead of falling back to data-retaining endpoints. The model picker offers only the reasoning effort levels advertised by the catalog, excluding `none` for models with mandatory reasoning. Leaving effort at the model default omits the override. Selected values are saved with the model and sent as `reasoning.effort`; `provider.require_parameters = true` prevents routing to endpoints that ignore requested parameters. See [OpenRouter ZDR](https://openrouter.ai/docs/guides/features/zdr) and [reasoning options](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens). History compaction uses Jev separately, as described below. Local tests use protocol fixtures and do not make paid OpenRouter calls.

Model registration uses an editable `modelprovider-modelname-reasoningeffort` name instead of the general entity default. For example, selecting `anthropic/claude-sonnet` with `high` suggests `anthropic-claude-sonnet-high`. Names follow model and effort changes until manually edited. The suffix uses the catalog's default effort when known, `default` when unspecified, and `none` for models without reasoning support.

### Migrating existing model registrations

Direct OpenAI and Anthropic inference configurations are no longer supported. Register a new OpenRouter model version using the catalog, then register agent versions referencing that model. Existing model versions without `max_output_tokens` retain their legacy output allowance; register a new version from the catalog to use the model's advertised maximum. Set `AIDASH_SECRET_OPENROUTER` on each server and worker. Existing OpenRouter entries keep their credential references and gain enforced ZDR automatically. Omitting `reasoning_effort` preserves the model default; non-ZDR fallback is not available.

The Compose Vite service proxies API requests to the backend container. When
running Vite directly on the host, set `AIDASH_BACKEND` to the node's URL; Vite
defaults to `http://127.0.0.1:8080` outside Compose.

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

`manage server` runs the API, outbox publisher and domain-event consumer. All roles publish durable Run activations, while only `manage worker` and `manage serve` pull from the shared activation consumer. Workers have four execution slots by default (`AIDASH_WORKER_SLOTS`, 1–4) without the management HTTP listener. Set `AIDASH_PROBE_LISTEN` to enable the separate health probe listener. `manage serve` runs both roles. Provision the activation stream once with `aidash activation-provision` using operator credentials and the same Node/namespace settings; runtime credentials do not create it by default. Broker outages retain database recovery. See [worker activation operations](docs/operations/worker-activation.md) for permissions, deadlines, diagnostics and verification. To exercise recovery, stop a **worker** process while leaving its server, PostgreSQL and NATS running, then restart it with the same configuration. The lease expires after 30 seconds. Task and run IDs remain stable.

## Tools and coordination

Every Agent retains `workspace_read` and `human_request`. The other Built-in tools are included by default and can be removed through `remove_default`. Cluster coordinators must retain `task_create`, `task_delegate` and `agent_discover`. The model's final text completes its task and publishes a final artifact. A coordinator must wait for its subtasks and synthesize their artifacts. Unresolved children require an explicit, audited abandonment before the parent can finish.

Tools are immutable Registry descriptors with provider-owned behavior and a JSON Schema for arguments. For an HTTP integration, register this configuration on its exact Node:

```json
{
  "registry_node": "aidash://node-a",
  "provider": "integration.http@1",
  "operation": "invoke",
  "default_alias": "research_lookup",
  "tier": "integration",
  "transport": {
    "transport": "http",
    "endpoint": "https://tools.example.com/research",
    "credential_env": "AIDASH_SECRET_TOOLS",
    "replay": "unsafe"
  }
}
```

Bind its exact registered identity in the Agent configuration:

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Research and publish findings.",
  "bindings": [
    {
      "kind": "tool",
      "target": {
        "registry_node": "aidash://node-a",
        "id": "research-http",
        "version": "1.0.0"
      },
      "alias": "research_lookup",
      "narrow": {}
    }
  ],
  "remove_default": []
}
```

The alias stays stable across versions. Run admission saves the complete dependency closure; subsequent steps use that snapshot while rechecking current resource authority. Memory and reference context require explicit Memory or Source bindings. Host packages, including shell and Python, require tenant approval and compatible Node providers. See the [Registry capability contract](docs/operations/registry-capabilities.md) for package preparation, atomic approval, lifecycle operations and the drained upgrade procedure.

MCP and Agent integrations use `integration.mcp@1` and `integration.agent@1` with their configuration under `transport`. An Agent integration creates and delegates a child of the invoking task. HTTP tools propagate `Idempotency-Key`, but HTTP/MCP behavior remains `Unsafe` without a verified provider contract: an uncertain effect pauses for reconciliation and is never automatically repeated. Transport replay labels do not weaken that rule. Native echo and native HTTP fetching are unsupported for ordinary new registrations.

## History compaction

Context compaction uses a Rust reimplementation of [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction), with a separate TypeSafe Jev connection. Set `AIDASH_SECRET_JEV` to your TypeSafe API key on each worker (or combined `serve` process). Optional `AIDASH_JEV_ENDPOINT` and `AIDASH_JEV_MODEL` default to `https://api.typesafe.ai/v1/systemone` and `jev-latest`. Jev is contacted only when the model's context budget is exceeded and old tool pairs can be pruned; those requests send a fitted history view to TypeSafe and use that account's credits.

Jev decides whether to keep each old tool call and its full result. Aidash keeps both, keeps the call with a shortened result, or removes the pair. It preserves the first and latest six history events, all human/non-tool events, and any legacy summary. Optional task/workspace/memory snapshot material is separately bounded to the model window with an explicit truncation marker. Workspace observations contain paged summaries and record IDs; agents use `workspace_read` for full details. Legacy observation results are converted to this view on replay, while durable audit records remain unchanged. It creates no new summaries and never silently falls back to summarization. Missing credentials, invalid decisions or insufficient reduction fail the step without changing its saved context. See the [compaction contract](docs/protocol.md#context-compaction) for input fitting and request limits. Protocol tests use a local Jev fixture; live service access and decision quality are not established by those tests.

## Generated API client

Reinhardt management routes and the existing Rust request/response schema types define the OpenAPI contract. The public `/api/openapi.json` endpoint and `manage openapi` command export the same document; the command needs no database, broker, or credentials. Other `/api` routes require a bearer token or an authorized dashboard session and selected context.

After changing an API route or type, run `scripts/generate-api.sh`. It exports `openapi/aidash.json` and runs the pinned Orval generator to update `web/src/generated/`. These outputs are ignored by Git, lint and coverage. The dashboard predev and prebuild scripts regenerate them automatically. Dashboard requests and types use these generated files; `transport.ts` supplies authentication/error handling, while `api.ts` reads and reconnects the SSE stream. The Orval transformer exposes unbounded `text/event-stream` responses as `Response`, so the browser can read frames without buffering the entire stream. Do not edit generated files by hand. CI generates them from a clean checkout before building and testing the UI.

## Verification

```sh
trunk fmt --all
trunk check --all --no-fix
scripts/check.sh
```

Trunk owns formatting and linting: rustfmt, Clippy, Prettier, ESLint, Ruff and Taplo. The Rust edition and linter versions are pinned. React Compiler is not enabled, so its incompatible-library diagnostic is disabled for the intentionally mutable TanStack Table/Virtual interfaces; the hook correctness and accessibility rules remain enabled.

PostgreSQL integration tests use `aidash-orm-test-postgres:17-pg-jsonschema-0.3.4`,
built by `scripts/build-test-postgres.sh` from the Dockerfile's `test` target.
The multi-node Compose fixture uses `aidash-compose-postgres:17-pg-jsonschema-0.3.4`
from its `runtime` target with separate database initialization. The distinct tags
keep Compose rebuilds from replacing the image used by isolated ORM fixtures.

`check.sh` runs Rust unit and PostgreSQL integration tests, builds the dashboard, and executes the two-node acceptance scenario starting from a real Chromium dashboard. It also verifies remote human controls, all four tool transports, and the browser scenarios. The scenario starts real Aidash processes, PostgreSQL and NATS with a deterministic OpenRouter-compatible protocol fixture. The same checks and Trunk lint run in GitHub Actions. Both nodes and workers start with NATS unavailable; the scenario verifies queued events drain after the broker connection is restored. It kills Node B's worker after an external effect but before its result is persisted, restarts the worker, and checks the same run completes with no duplicate effect. Reports are written to `.ignore/acceptance/report.json`.

Golden Path also requires subject-scoped remote execution on these real Node processes. Separate source and receiver tenants, credentials, catalog approvals and peer mappings authorize the work. It kills the receiver Worker after a Home message or task completion commits but before its response is delivered, checks recovery of the same admission and Run without duplicate effects, and repeats recovery with a peer outage and with a grant revoked while the Worker is stopped. Cross-tenant/workspace access, disabled Agents, unauthorized dependencies and legacy admission are rejected. The `scoped_remote_execution` report records each scenario, the Worker process IDs and persistent execution IDs without credentials. A successful legacy scenario alone cannot pass this gate.

Use `scripts/test-acceptance.sh` to build the current checkout's binary and frontend before running the full gate. The report includes the Git revision, dirty status, source fingerprint and binary digest; changing the source or binary while the gate runs fails verification. The browser authentication fixture exercises real Bearer-authenticated APIs; it does not establish an OIDC provider login flow.

The script adds `server/src/apps/execution/tests/fixtures/acceptance.compose.yaml` to give its disposable PostgreSQL service 400 connections for the two API processes and two workers. When running Golden Path directly, start Compose with `COMPOSE_FILE=compose.yaml:server/src/apps/execution/tests/fixtures/acceptance.compose.yaml` so those independent pools have the same capacity.

For browser tests, keep that completed fixture environment running in one terminal:

```sh
cargo build --locked -p aidash-server --bin manage
python3 scripts/golden_path.py --binary "$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"] + "/debug/manage")')" --keep
```

In another terminal:

```sh
(cd web && npm exec -- playwright install chromium)
npm test --prefix web
```

Protocol fixtures verify transport, coordination and recovery. They do not establish live model answer quality or provider-account availability. No commercial model calls are made by these tests.

## Database migrations

Reinhardt owns the app migration graph under `server/migrations/<app_label>/`. The native baseline supports empty PostgreSQL databases and replay of its own history; existing SeaORM or experimental migration databases are not adopted. Run `cargo run --locked -p aidash-server --bin manage -- migrate` through the native management CLI. See the [migration correspondence and maintenance cutover procedure](server/migrations/README.md) before switching deployments.

The PostgreSQL image supplies the `pg_jsonschema` library. Native history creates and owns an absent extension. For a database-scoped, non-superuser application role, an administrator provisions the extension first; rollback preserves that borrowed extension. Use an empty database template. The Compose `local-dev-db` preparation command applies this same history and validates the extension version before the backend starts.

## CI and coverage

CI runs Trunk, Clippy, eight Rust test partitions, a separate coverage upload, Bruno API contracts, infrastructure checks, collaboration browser tests, and the PostgreSQL/NATS/Chromium acceptance suite in separate jobs. Each Cargo test target belongs to exactly one partition; the inventory is derived from locked Cargo metadata and validated before execution. Dependency caches are isolated by partition. All eight LCOV reports are required and uploaded together under the same Rust coverage flag. `CI Success` requires every CI job to succeed, including the Codecov upload. Use that check for branch protection. Rust coverage uses `cargo llvm-cov` with real PostgreSQL tests and uploads an explicit LCOV file through Codecov OIDC. Codecov measures `crates/` and `server/src/`; tests, migration plumbing and generated API files are excluded. Browser tests establish dashboard behavior and are not included in the Rust coverage percentage.

Cluster recovery, transaction cluster recovery, and isolated capability runtime acceptance are available for manual execution. Run `bash scripts/test-cluster.sh kubernetes` and `bash scripts/test-cluster.sh k3s` after installing the browser dependencies; use the `transactions` or `remote-memory` profile for those recovery suites. Run `scripts/test-capability-cluster.sh` for isolated capability acceptance. These suites are outside the CI workflow and its `CI Success` prerequisites.

The coverage uploader uses a pinned Codecov CLI from PyPI; see the [download outage and recovery condition](docs/operations/ci-coverage.md).

Run `scripts/test-rust.sh --partition identity` for one partition, or `python3 scripts/rust-test-partitions.py --check` to inspect the complete inventory. Run `scripts/test-rust.sh --coverage` to produce `coverage/rust.lcov` locally (requires `cargo-llvm-cov` 0.8.7 and `llvm-tools-preview`). `scripts/check.sh` runs the full local suite. Cargo and npm lockfiles remain tracked for reproducible dependency resolution.

Run `npm exec --yes --package=@usebruno/cli@3.1.3 -- scripts/test-bruno-api.sh` with the test PostgreSQL and NATS services running to verify the real HTTP API. The [Bruno collection](server/tests/bruno/README.md) checks all 269 native endpoints with 3–10 scenarios each, including scoped authorization, input rejection, state changes, browser cookies/CSRF, finite SSE replay and frontend caching against a disposable database and the compiled server. Sanitized reports include the source revision and executable hash.

Package installation overlays the supplied node-local configuration onto the entity configuration, validates it, and publishes the effective immutable Registry version atomically with the installation record. Changing an installed configuration requires a new version.

Third-party attribution for the adapted context compaction code is in [LICENSE](LICENSE).

HTTP request logging, admission limits, SSE capacity, and the optional Prometheus
listener are documented in [HTTP protection and observability](docs/operations/http-observability.md).

## Desktop client

The [Tauri 2 desktop client](desktop/README.md) bundles the shared dashboard and connects to an existing local or remote Aidash server. It supports external-browser Google sign-in and persistent OS-protected credentials.
