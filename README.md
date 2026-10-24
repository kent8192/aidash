# Aidash

Aidash 0.1 is a self-hosted platform for federated agents. Register an explicitly selected model and an agent, start a goal in the dashboard, and let agents claim work across Nodes, including Nodes of other independently operated Deployments. Workspaces retain tasks, artifacts, messages and an ordered event log. Workers persist their execution state and recover after process termination.

The current implementation includes Federation between Nodes, scoped remote Worker activation and Home commands, scoped authorization, policy-driven local agent generation, a recoverable cross-Node transaction protocol, persistent semantic memory, Creator and Trust workbenches, and Kubernetes deployment. See [architecture](docs/architecture.md), [authorization](docs/authorization.md), [generation](docs/generation.md), [distributed transactions](docs/transactions.md), [orchestration](docs/orchestration.md), and [protocol and recovery contracts](docs/protocol.md). Source and focused test coverage for these paths do not close the full scoped federation (#38), transaction (#40), A2A (#39), or combined release acceptance gates.

New multi-Node scoped transactions are temporarily rejected to prevent manifest disclosure after grant revocation. Previously admitted transactions continue durable recovery; see the [transaction contract](docs/transactions.md).

## Cargo workspace

Native server builds require the Protocol Buffers compiler (`protoc`) on `PATH`
for `reinhardt-grpc`. The provided Docker images install it in their build stage;
the standalone browser frontend builds do not require it.

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

On Unix, `server` and `serve` can receive an owned listening socket through
`AIDASH_LISTEN_FD`, validated against `AIDASH_LISTEN`. The
[transaction process fixture](docs/operations/transaction-acceptance.md) uses
this handover to keep its endpoints reserved across process restarts.

| Package               | Responsibility                                                           |
| --------------------- | ------------------------------------------------------------------------ |
| `aidash-domain`       | Business models, typed state, and pure invariants.                       |
| `aidash-application`  | Authorized use cases, shared execution contracts, and external ports.    |
| `aidash-harness`      | Agent steps, worker activation, leases, cancellation, and recovery.      |
| `aidash-runtime`      | Background supervision, shutdown, and worker drain.                      |
| `aidash-integrations` | Inference, HTTP/MCP, NATS and Kubernetes adapters.                       |
| `aidash-server`       | Reinhardt HTTP/ORM/settings, app repositories/migrations, and bootstrap. |

`server/src/bootstrap.rs` assembles the concrete adapters for both HTTP and
workers. Apps are ordinary Rust modules. The harness depends on application/domain
contracts; application/domain do not depend on the harness. Domain/application/harness
production dependency closures exclude runtime, integrations, the server, Reinhardt,
Axum, SQLx, SeaORM, and reqwest; CI checks these boundaries against locked Cargo metadata.

### Community and Enterprise builds

The packages above form the **Community build**, which runs only the
`self-hosted` Offering. `enterprise/` is a separate Cargo workspace with its own
`Cargo.lock`. It composes `aidash-server` into the **Enterprise build**, which
runs only the `cloud` and `dedicated` Offerings. Each Deployment selects its
Offering in `[deployment] offering` of its settings, and `self-hosted` is the
default. A build refuses to start with an Offering it does not run.

```sh
cargo test --locked --manifest-path enterprise/Cargo.toml
cargo run --locked --manifest-path enterprise/Cargo.toml --bin aidash -- serve
```

The Enterprise server needs `offering = "cloud"` in `server/settings/local.toml`.
See [Community and Enterprise builds](docs/operations/community-and-enterprise-builds.md)
for the layout, the alignment and export checks, and the images.
The React dashboard remains in
`web/` and uses the existing URL, JSON, authentication, and SSE contracts.

Reinhardt's exact revision is pinned in the [workspace manifest](Cargo.toml),
aligned with Enterprise and the infrastructure observer, and retained in their lockfiles. The initial
project and app scaffolds were generated with the Reinhardt CLI. Server tests use
its composable guard, method-aware stub router, configurable PostgreSQL fixture,
and request builder with per-request headers and redirect policy. The PostgreSQL
17 extension image remains project-specific. Shared JetStream setup uses the native
Send fixture after reinhardt-web#6705 fixes #6702. Native ORM timestamp bindings
include the #6703 repair from #6704; both issues remain open until closed by the
maintainer because their fixes merged into the development branch. Streaming
clients retain no total timeout while reinhardt-web#6689 remains open.

### Standalone authentication and Marketplace

The Enterprise workspace also contains two independent services:
[shared authentication](enterprise/auth/backend/README.md) and
[Marketplace distribution](enterprise/marketplace/backend/README.md).
Each has its own process, configuration, schema, database role and migration
command. They start without the Cloud server, worker or NATS. A physical
PostgreSQL instance may be shared with separate ownership and credentials.

The [Marketplace web app](enterprise/marketplace/frontend/README.md) has its own build
and container; the [authentication web app](enterprise/auth/frontend/README.md) is
bundled with its service image. See the [independent deployment chart](enterprise/deploy/standalone-service/README.md)
and [approved authentication/distribution design](docs/authentication-and-standalone-marketplace-design.md).
Cloud enables shared authentication explicitly through `[dashboard.common_auth]` and
`[dashboard.oidc]`; direct `[dashboard.gcip]` sign-in is mutually exclusive.
Dedicated and Self-hosted keep independent sign-in. Import retained legacy
Identity UUIDs before the Cloud cutover; Marketplace sign-in creates no Cloud Tenant.

## Run locally

### One-command Kubernetes start

With Docker, kind, kubectl, Helm, curl, Python 3, and cargo-make installed, run:

```sh
cargo make k8s-up
```

This creates a dedicated `aidash-local` kind cluster, builds and imports the
Aidash backend, frontend, and PostgreSQL images (including `pg_jsonschema`),
starts persistent PostgreSQL with pgvector/PGroonga and JetStream NATS, deploys the server,
worker, and frontend with Helm, and exposes the dashboard at
<http://127.0.0.1:8080>. The frontend Service serves the dashboard and proxies
API, authentication, federation, and health requests to the internal backend Service.

The server and worker use a PostgreSQL [Secret Store](docs/provider-credentials.md).
Startup generates its Master Key and fingerprint key once in the `aidash-local-app`
Secret, so Tenants can connect OpenRouter under **Provider connections** without
manual Store setup. Re-running `k8s-up` preserves both keys; `k8s-down` deletes
them together with the local database. Partial or invalid keys stop startup
instead of replacing keys that may protect existing credentials.

Before starting the application, a one-shot Job provisions the Node's JetStream
activation stream and durable worker consumer. Re-running `k8s-up` safely
reuses them; a failed provisioning Job stops startup.

The built-in loopback endpoint is plain HTTP, so it does not support browser sign-in; the dashboard shows setup guidance, while `AIDASH_API_TOKEN` remains available for the API and initial Deployment Administrator grant. Browser sessions require a trusted HTTPS dashboard origin. To enable OIDC, publish the frontend Service and its API/auth proxy through HTTPS, add `AIDASH_OIDC_CLIENT_ID`, `AIDASH_OIDC_CLIENT_SECRET`, and that HTTPS `AIDASH_OIDC_PUBLIC_ORIGIN` to the `aidash-local-app` Secret, then re-run `cargo make k8s-up` to restart the Pods. Register the matching `https://<dashboard-origin>/auth/callback` URI in Google Auth Platform. Google endpoints must be reachable from the backend Pod. Add provider credentials to this Secret as needed:

```sh
kubectl --kubeconfig private/local-k8s/kubeconfig -n aidash-local edit secret aidash-local-app
```

The task keeps a dedicated kubeconfig in `private/local-k8s/` and does not
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
Recreate it the same way if its database predates the `aidash_owner` and
`aidash_server` roles (see [Database roles](#database-roles)).

### Docker Compose development

Prerequisites: `cargo-make` and Docker Compose v2.24 or later. Rust 1.96
and Node.js 24.12.0 run inside the development images. Start PostgreSQL, NATS,
the backend and Vite in detached mode:

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
and an HTTPS endpoint for a deployed Deployment.

Compose waits for JetStream readiness and provisions the activation stream and
durable consumer before starting the backend. The stream is recreated after
`docker compose --profile dev down -v`, so a fresh stack exercises broker
activation instead of relying on database recovery alone.

The backend starts with a PostgreSQL [Secret Store](docs/provider-credentials.md),
so a Tenant can connect OpenRouter under **Provider connections**. Compose
generates the Store's Master Key and fingerprint key once, in the
`provider-credentials` volume. `docker compose --profile dev down -v` deletes
them together with the database that holds the encrypted credentials.

The Compose backend admits core work through a [capability execution profile](docs/operations/core-capabilities.md)
with the default limits, so Goals for Agents with core capabilities run instead
of failing with `CAPABILITIES_DISABLED`. Working files live in the
`capabilities` volume. The stack has no isolated Runner, so Sandboxed tools
such as shell and Python stay unavailable.

The Compose backend also turns on Personal Tenants (`[tenancy] personal_tenants = true`,
mounted as its `local.toml`), so your first Google sign-in gets a Personal
Tenant without approval. Other Self-hosted deployments keep the Offering's default, `false`
(see [Tenancy Rules](docs/authorization.md#tenancy-rules-tenant-creation-and-invitations)).

To sign in without a Google client, first expose the frontend and its API/auth
proxy at a trusted HTTPS origin and set that origin in `.env`, together with
this Compose override:

```dotenv
COMPOSE_FILE=compose.yaml:deploy/local-oidc/compose.yaml
AIDASH_OIDC_PUBLIC_ORIGIN=https://dashboard.example.test
```

Run `cargo make dev` to start a generic local OIDC issuer and its loopback relay
with the development stack, and to point the backend at it. The HTTPS proxy must
route the dashboard, `/api`, and `/auth` to the Compose frontend without changing
the configured public origin. The issuer signs in whatever subject you type on
its page, so it is for local development only. The relay makes
`http://127.0.0.1:18099` reachable inside the backend's network namespace, so
the browser and backend use the same issuer. Both use the official Node.js image
independently of the frontend build, including on the first start or with a
custom `COMPOSE_PROJECT_NAME`.

Demo data (fictional Tenants, subjects, Registry entries and Catalog Approvals) loads by hand into the running backend. Each Demo Tenant receives a default Node, where its Catalog Approvals apply. The command refuses unless `[demo_data] enabled` is set, which the Compose `local-settings` config does:

```sh
docker compose --profile dev exec backend aidash demo-data \
  --from /app/deploy/demo-data/structure.json \
  --people /app/deploy/demo-data/local-people.json
```

Each input digest loads once; a rerun with unchanged inputs prints `Demo data unchanged`. The local people file reserves `alex@example.test` for the `acme` administrator and `globex` member Demo Tenants, and `taylor@example.test` for a member of `acme`. Sign in with the local issuer above as subject `alex` to land in them. Demo data holds no Provider Credentials, so a Demo agent runs only after a Tenant Administrator connects OpenRouter with **Connect OpenRouter** and binds the resulting Provider Credential. Environments (develop, `pr-N` and test) load Demo data automatically; see [infra/gcp/README.md](infra/gcp/README.md).

The [authorization API](docs/authorization.md) issues revocable subject tokens for tenant-scoped workspaces, approved Registry discovery, local agent execution and event streams. Workers recheck the root and delegated agents at every durable boundary. Where the Tenancy Rules turn Personal Tenants on, Google sign-in creates personal Tenant authority; otherwise a new identity files a registration request. Shared Tenant membership requires approval and Deployment Administrator access requires a separate explicit grant. Its selected authority is local to each tab. Existing API Bearer credentials remain available. Deployment Administrators use **Access policies / アクセス制御** to edit role/attribute policies, simulate decisions, approve registration requests, manage mappings and Deployment Administrator grants, and issue or revoke subject credentials. Scoped remote federation remains under implementation.

### Aidash Cloud GCIP sign-in

Cloud and Dedicated deployments can select GCIP as the sole dashboard issuer. GCIP sign-in is part of the Enterprise build, which reads `[dashboard.gcip]`; the Community build does not know the section. `[dashboard.gcip]` and `[dashboard.oidc]` are mutually exclusive. Self-hosted OIDC stays optional and unchanged. Add this section to the selected server settings file:

```toml
[dashboard.gcip]
project_id = "aidash-cloud-project"
web_api_key = "PUBLIC_FIREBASE_WEB_API_KEY"
public_origin = "https://develop.aidash.run"
auth_helper = "public_origin"  # default "firebase"
session_absolute_seconds = 43200
session_idle_seconds = 1800
shared_pool = "shared-pool-id"
password_sign_up = ["shared-pool-id", "acme-pool-id"]

[dashboard.gcip.tenant_bindings]
acme-pool-id = "acme"

[dashboard.gcip.providers]
shared-pool-id = ["google.com", "password"]
acme-pool-id = ["oidc.acme"]

[dashboard.gcip.sign_in_domains]
"acme.example" = "acme-pool-id"
```

`shared_pool` names at most one Shared Pool: a GCIP Tenant with no Tenant Binding, whose identities follow the Tenancy Rules. Tenant Bindings are optional and map a GCIP Tenant one-to-one to an Aidash Tenant name, for a Tenant that brings its own single sign-on. Startup refuses a Shared Pool that is also bound, and a Binding that names a Tenant without a root record or a Personal Tenant. A GCIP Tenant that is neither shared nor bound admits nobody, and its identities are disabled. An identity whose GCIP Tenant changes between shared and bound is disabled until a Deployment Administrator restores it. Sign-in through the pool of a deleted bound Tenant is refused with `410 tenant_deleted`. Terraform supplies these IDs, provider choices, signup settings and Sign-in Domains to the environment runtime secret. The runtime uses Application Default Credentials for tenant-scoped `accounts:lookup`; it never uses the public web API key as an administrative credential. See [GCIP infrastructure setup](infra/gcp/README.md#gcip-sign-in) and [authorization](docs/authorization.md#gcip-sign-in).

The lazy `/sign-in` page asks for an email first. A form `POST /auth/login` routes a Sign-in Domain (a verified email domain, lowercase ASCII with punycode for IDN) to its bound GCIP Tenant, and every other address to the Shared Pool. Either creates a browser-bound ten-minute transaction. Without a Shared Pool, unknown domains and malformed addresses receive one identical response. Sign-in Domains only route; they never grant authority. The npm Firebase Auth SDK uses memory persistence, popup federation and an email/password form; signup waits for verified email. When the browser blocks the popup, federation falls back to a redirect that returns to the same transaction. The backend verifies signed ID tokens and live Account Status, then exchanges them for the existing opaque HttpOnly Aidash session. GCIP tokens are discarded and are never put in browser storage; every attempt also removes any `firebase:` storage keys and Firebase's IndexedDB store. A Tenant whose MFA Requirement is `enabled` or `mandatory` ([tenant MFA](infra/gcp/README.md#tenant-mfa)) makes the same page ask for the 6-digit code of the user's authenticator app after the first factor, for popup, password and redirect sign-in alike. A user with no factor on a `mandatory` Tenant is led through TOTP enrollment (setup key and `otpauth://` link) before the exchange, and the account menu offers the same enrollment as "Set up two-step verification" by signing in again through the session's own GCIP Tenant (`GET /auth/login` with a valid session), because Aidash keeps no GCIP user between attempts; a user who already holds a factor answers its challenge first and then adds the new authenticator. GCIP enforces the Requirement; the exchange verifies the token exactly as before and never reads second-factor claims as authority. Registration approval, Mappings and Deployment Administrator grants remain Aidash authority.

`auth_helper` selects the origin that serves Firebase's sign-in helper. The default, `firebase`, uses `<project_id>.firebaseapp.com` and needs no proxy, but provider consent screens show that domain and redirect sign-in fails in browsers that block third-party storage. `public_origin` uses the `public_origin` host as `authDomain`. Choose it only after the public origin transparently proxies `/__/auth/*` and `/__/firebase/init.json` to `https://<project_id>.firebaseapp.com` without forwarding Aidash cookies, for example with nginx:

```nginx
location ~ ^/__/(auth/|firebase/init\.json$) {
  proxy_pass https://<project_id>.firebaseapp.com;
  proxy_ssl_server_name on;
  proxy_set_header Host <project_id>.firebaseapp.com;
  proxy_set_header Cookie "";
  proxy_set_header Authorization "";
  proxy_hide_header Set-Cookie;
}
```

Then register `https://<public origin host>/__/auth/handler` as each identity provider's redirect URI (the SAML ACS URL for SAML providers). Aidash Cloud environments switch to `public_origin`, and their SAML ACS URL, with the deployment that installs the Caddy proxy.

### Dashboard OIDC setup

Self-hosted Aidash signs in through any OpenID Connect issuer, directly with Google by default, using Authorization Code with PKCE. The backend verifies the ID token's signature, issuer, audience, expiry and nonce before creating an HttpOnly session; it never exposes the issuer's tokens or the client secret to the browser. Identity is keyed by issuer and `sub`, never by email. The steps below use Google; for another issuer, set `AIDASH_OIDC_ISSUER` to its issuer URL and register the same redirect URI with it.

1. In [Google Auth Platform](https://console.cloud.google.com/auth/overview), configure your application's branding and audience. Add permitted test users while the app is in testing.
2. Create an OAuth client of type **Web application**. Register the exact authorized redirect URI `<origin>/auth/callback`, for example `https://dashboard.example.test/auth/callback`.
3. Set `AIDASH_OIDC_CLIENT_ID`, `AIDASH_OIDC_CLIENT_SECRET`, and `AIDASH_OIDC_PUBLIC_ORIGIN` on the backend as shown in `.env.example`, then restart it. `AIDASH_OIDC_ISSUER` defaults to `https://accounts.google.com`. Keep the client secret on the backend only.

The Vite proxy forwards `/auth` to the backend. Every dashboard origin that enables browser sign-in must use trusted HTTPS, which enables Secure host-only session cookies. Upgrading from a release that allowed loopback HTTP signs existing browser sessions out once; desktop credentials are unaffected. Sessions expire after 12 hours or 30 minutes without user activity by default. See [Google's OpenID Connect documentation](https://developers.google.com/identity/openid-connect/openid-connect) for provider setup.

Aidash reads no account status from an OIDC issuer: a valid ID token is the whole admission. An issuer may still send back-channel logout notifications to `POST /auth/backchannel-logout`; Google sends none. Account changes at the issuer therefore do not immediately revoke existing Aidash sessions or pause admitted work. Aidash continues checking local identity disablement, subject policy, mappings and credentials at execution boundaries; use local mapping/deployment-administrator-grant revocation to remove access. Aidash logout affects Aidash sessions, not the Google browser session. No offline Google access or refresh token is requested.

Switching issuers creates separate external identities: mappings and Deployment Administrator grants of the old issuer are not transferred to the new one, even if email addresses match.

Changing issuers requires a coordinated stop/start, including when switching back. Stop every Aidash server and worker that shares the database, update all replicas to the same issuer configuration, and then restart them before allowing sign-in. Do not use a rolling restart: a process still using the old issuer can permanently disable identities and revoke sessions created under the new issuer. See the [issuer-switch procedure](docs/authorization.md#switching-issuers) before migrating.

When `[tenancy] personal_tenants` is `true`, a verified Google sign-in provisions one personal Tenant and user subject for that external identity. With `false`, the Self-hosted Offering's default, a new identity instead files a registration request for a Deployment Administrator to approve. With a personal Tenant, personal use requires no registration request: the dashboard selects the personal Mapping when no valid authority is saved for the tab. The allocation is tied to the verified provider identity, not its email or display name, and later sign-ins reuse it. Existing shared Tenant Mappings and explicitly granted Deployment Administrator access remain separate choices.

Google users with a personal Tenant can join a shared Tenant by choosing **Request access to shared tenant** in **Account settings**. A Deployment Administrator approves the authenticated request against an existing enabled user subject in **Access policies**; a pending request does not interrupt personal use. Shared approval never grants Deployment Administrator access. For the first Deployment Administrator grant, list verified identity IDs with `GET /api/dashboard/identities` using the existing Deployment Administrator Bearer token, then grant the selected identity Deployment Administrator access with `POST /api/dashboard/identities/{id}/deployment-administrator-grant` and body `{"enabled":true,"expected_revision":0}`. GCIP sign-in retains its enrollment and approval flows, and Aidash checks its Account Status at most 15 minutes after the last successful check; a definite disablement revokes browser sessions and pauses work. Once the account is enabled again, a Deployment Administrator must explicitly call `POST /api/dashboard/identities/{id}/restore`; previously revoked sessions require a new sign-in. Current-browser and all-device Aidash logout are separate dashboard actions. API Bearer recovery remains available during IdP outages.

Every Tenant has its own Registry, which only its subjects reach. The **Registry** screen can register models, tools, skills, clusters, agents, compaction Deciders and embedding providers for a subject with `registry.create`, and a subject with `catalog.approve` approves each version before use; approving an agent also makes it runnable. Register a model before an agent. Inference uses OpenRouter `/chat/completions`. Specify the provider's actual model ID, context window, modalities and cost metadata. Model and embedding configurations never reference an environment key (ADR 0006): they either name the Provider Catalog entry `provider_credential: "openrouter"`, which uses the Tenant's connected OpenRouter Provider Credential, or call their endpoint without a key. Such a Keyless endpoint may connect only to public addresses unless the Deployment permits its exact origin in `[keyless_endpoints] permitted_private_origins` or `AIDASH_KEYLESS_PRIVATE_ORIGINS` (see [Keyless endpoints](docs/provider-credentials.md#keyless-endpoints)). `AIDASH_SECRET_*` references remain in Deployment settings for declared Research providers and runners. Tenant tools and sandboxed code use [Tenant Secrets and External services](docs/tenant-secrets.md). Compaction Deciders name `provider_credential: "openrouter"` and an exact Jev release such as `typesafe/jev-1.14`. With a configured write-only Secret Store, Tenant settings can inspect metadata, revoke, bind, unbind and delete [Provider Credentials](docs/provider-credentials.md). Key Material is obtained only through Provider Authorization: **Provider connections** in Tenant settings runs OpenRouter's OAuth PKCE flow on the server (Connect, Reconnect) and never shows or accepts the key itself. Registry configs use the closed Catalog ID `provider_credential: "openrouter"`. Admitted Runs pin a Tenant-owned Provider Credential ID. With a self-hosted PostgreSQL Store, Tenant provider calls read that Credential's pinned version locally after metadata validation. With a Cloud Secret Manager Store, Tenant provider calls fail closed after metadata validation until the credential broker from Issue #137 is configured. Model selection is explicit; Aidash does not select fallback models or automatically route between models.

Cloud environment retirement deletes its dynamic BYOK secrets before removing the data disk and runtime identity. Human-run bootstrap grants deploy only prefix deletion and project inventory for this cleanup, preserving the Create/Manage-only IAM grant boundary. Deploy remains the trust root under #151 because it already controls the delete-bearing Manage binding; project inventory makes secret names visible to it. See the [infrastructure trust boundary](infra/gcp/README.md).

Registration starts with an editable English `adjective-animal` name and one description field. The server assigns a UUID v7 internally; the registration form does not expose an ID field. The form retains a private request key for unchanged retries, and the server saves its assigned ID atomically with registration. API clients can still supply explicit IDs or send an `Idempotency-Key` UUID header; reusing a key with different input returns a conflict. An ID and version together identify the immutable registration. The form stores name and description in `en` and provides editable discovery languages defaulting to `ja`/`en`. Skills can import existing `SKILL.md` instruction files. Agent creation prioritizes selected Skills, optional additional instructions, and personal PDF/Excel/text references; see [Skills-first agents](docs/features/skills-first-agents.md) for supported formats and execution limits. Clusters select an exact coordinator-agent version, and tools have connection-specific fields plus an argument editor for text, numbers, booleans, groups and lists. Argument editors preserve allowed values, patterns and numeric bounds. Native web tools provide the URL argument automatically. Agent tools use fixed task arguments: required title and description, with optional requirements, dependencies and parent task. No configuration JSON is entered. Authenticated HTTP/MCP tools name an optional Tenant Secret with a recorded destination host; OpenRouter embeddings use the Tenant's OpenRouter Provider Credential, OpenAI-compatible embeddings are called without a key (a private embedding server needs its origin permitted), and compaction Deciders use the Tenant's OpenRouter Provider Credential too. A subject needs `secret.use` to name a Tenant Secret and `secret.manage` to change its value or permitted hosts.

Embedding registration and new semantic indexes default to OpenRouter `https://openrouter.ai/api/v1`, model `google/gemini-embedding-2`, model version `1.0.0`, and **3072 dimensions**. The adapter sends the approved `dimensions`, float encoding, and `provider.zdr = true` to `/embeddings`, and validates the returned model and vector dimensions before accepting the result. Smaller dimensions can reduce database storage and similarity-computation cost, but may reduce retrieval quality and do not reduce API input charges. Workspace index generations and pinned bank policies must use the same embedding configuration. Index updates validate every pinned bank, including empty banks, and bank policy changes validate the existing index. Pinned banks also require the index to stay enabled. Compatible settings updates can rebuild a generation. To adopt a different embedding, register and approve its definitions and configure a fresh Workspace index before creating its banks; existing banks retain their pinned embedding until an atomic migration is supported. Model version is an embedding generation label set by whoever configures the index (a subject holding `semantic.index.manage`, which `members` hold, or a Deployment Administrator); change it when the upstream model contract changes. The current native memory ingestion and retrieval inputs remain text; selecting a multimodal model does not by itself add image ingestion or image RAG. See [OpenRouter embeddings](https://openrouter.ai/docs/api_reference/embeddings).

For **OpenRouter**, connect the Tenant under Settings → Provider credentials → Provider connections and bind the resulting Provider Credential. The Registry model form uses OpenRouter: search the live model catalog by name or ID and select a text model with tool calling. Aidash fills the model ID, endpoint, context window, maximum output tokens and indicative per-million-token pricing automatically. Models without a published maximum output limit are not offered. Any signed-in member reads the catalog through `GET /api/providers/openrouter/models`, which answers `{models, stale, fetched_at}`. The server fetches it at most once every ten minutes for everyone; while OpenRouter fails it serves the last catalog, marked `stale`, for up to a day, and otherwise answers 502. Catalog failures can be retried from the form. No credential reference is entered in the model form; models register with `provider_credential: "openrouter"`.

The model's advertised maximum output is saved in its immutable Registry version and used for each inference request, context-fit check and generation reservation. OpenRouter requests use Bearer authentication and `max_tokens`. Every inference request enforces `provider.zdr = true`; a model without an available ZDR endpoint fails instead of falling back to data-retaining endpoints. The model picker offers only the reasoning effort levels advertised by the catalog, excluding `none` for models with mandatory reasoning. Leaving effort at the model default omits the override. Selected values are saved with the model and sent as `reasoning.effort`; `provider.require_parameters = true` prevents routing to endpoints that ignore requested parameters. See [OpenRouter ZDR](https://openrouter.ai/docs/guides/features/zdr) and [reasoning options](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens). History compaction uses Jev separately, as described below. Local tests use protocol fixtures and do not make paid OpenRouter calls.

Model registration uses an editable `modelprovider-modelname-reasoningeffort` name instead of the general entity default. For example, selecting `anthropic/claude-sonnet` with `high` suggests `anthropic-claude-sonnet-high`. Names follow model and effort changes until manually edited. The suffix uses the catalog's default effort when known, `default` when unspecified, and `none` for models without reasoning support.

A model version can declare `cache_mode`: `none` (the default), `automatic` for routes that cache matching prefixes on their own, or `explicit` for routes that cache only up to `cache_control` breakpoints. `explicit` is accepted only for `anthropic/` model IDs. An Agent on the `ordered` projection and such a model can set `prompt_cache: explicit`; its requests then mark the end of the system prompt and of the Stable Prefix part. Cache writes cost more than ordinary input, so enable it for Agents whose Runs take several steps. Message catch-up and media-deferral steps carry no breakpoints, and no other route ever receives `cache_control`. Both fields are set in the Registry API's JSON config; see [Context Projection Versions](docs/features/context-projection.md#explicit-prompt-caching).

### Migrating existing model registrations

Direct OpenAI and Anthropic inference configurations are no longer supported. Register a new OpenRouter model version using the catalog, then register agent versions referencing that model. Existing model versions without `max_output_tokens` retain their legacy output allowance; register a new version from the catalog to use the model's advertised maximum. OpenRouter calls always enforce ZDR. Omitting `reasoning_effort` preserves the model default; non-ZDR fallback is not available.

**Upgrading past #158 (breaking):** `credential_env` and `AIDASH_SECRET_OPENROUTER` / `AIDASH_SECRET_EMBEDDING` no longer configure model or embedding providers. Registry model and embedding entries, their installation overrides, and semantic index specs that contain `credential_env` no longer load, and the `registry/0024_model_provider_keys` and `knowledge/0031_embedding_provider_keys` migrations refuse to run while such records exist. Delete those entries or remove the key from those overrides (or reset the development database), connect OpenRouter for each Tenant through Provider connections, then register new versions with `provider_credential: "openrouter"` (or without a key for an unauthenticated endpoint). Jev moved to OpenRouter as well: `AIDASH_SECRET_JEV`, `AIDASH_JEV_ENDPOINT` and `AIDASH_JEV_MODEL` are ignored, `compactor` entries drop `provider`, `endpoint` and `credential_env` for `provider_credential: "openrouter"` and a `typesafe/jev-X.Y` model, and `registry/0025_compactor_provider_keys` refuses to run while an old compactor exists. Ordinary Runs are no longer pruned (see History compaction).

**Upgrading past #108 (breaking):** the `compactor` Registry kind is gone, and `registry/0035_decider_kind` refuses to run while a compactor entry exists. Register a `decider` entry instead (an exact `typesafe/jev-X.Y` release, its mode, per-Run call limit and state budget), approve its exact version in each Node's tenant catalog, and bind it to the Agents that should be pruned. Generation policies and RequiredHome remote execution name Decider versions where they named compactors, and the `compaction.invoke` action is now `decision.invoke`. Decision Evidence moved to version 3; `execution/0036_decision_evidence_v3` refuses to run while version 2 Decision Attempts exist. The `prune_unavailable` pause reason is replaced by `decider_unapproved`, `decider_calls_exhausted` and `decider_unavailable`.

**Upgrading past #153 (breaking):** a `decider` configuration lists its models as ordered `routes` (ADR 0066). Move `provider_contract`, `model`, `threshold` and `state_budget` into one entry of `routes` (#412 renamed `keep_threshold` to `threshold`); an entry may add a later Route such as `{"provider_contract":"openrouter.decisions/1","model":"cloudflare/clef","hosts":["cloudflare","deepinfra","primeintellect"],...}`. Decision Evidence moved to version 4 and records the Route it asked. `execution/0044_decision_routes` refuses to run while a Decision Attempt exists.

**Upgrading past #256 (breaking):** a model or embedding configuration without `provider_credential` now connects only to public addresses. An existing one that points at a private, loopback or in-cluster server, such as a local Ollama, stops with an authority failure until a Deployment Administrator lists its origin in `[keyless_endpoints] permitted_private_origins`, `AIDASH_KEYLESS_PRIVATE_ORIGINS` or the Helm value `keylessEndpoints.permittedPrivateOrigins`.

**Upgrading past #376 (breaking):** Tenant HTTP and MCP tool endpoints, and the remote MCP servers of Agent Plugins, now follow the same rule and share the same list. A tool registered at a private, loopback or in-cluster address is refused when it is called until a Deployment Administrator lists its origin, and registering one is refused with `TOOL_ENDPOINT_FORBIDDEN`. A host name needs HTTPS unless its origin is listed.

The Compose Vite service proxies API requests to the backend container. When
running Vite directly on the host, set `AIDASH_BACKEND` to the backend's URL; Vite
defaults to `http://127.0.0.1:8080` outside Compose.

## Two Deployments

Compose creates `aidash_a`, `aidash_b`, and `aidash_test`. Deployment B needs its own database, identity and port:

```sh
DATABASE_URL=postgres://aidash_server:aidash-server-local@127.0.0.1:54370/aidash_b \
AIDASH_DEPLOYMENT_ID=aidash://node-b \
AIDASH_ENDPOINT=http://127.0.0.1:8081 \
AIDASH_LISTEN=127.0.0.1:8081 \
cargo run --locked -- serve
```

Nodes of one Tenant reach each other under the Tenant's policy (`node.use`) and need no registration. To let a Node of another Tenant, in this Deployment or another one, exchange work with a Node of yours, a Tenant Administrator registers it on **both** sides with `POST /api/tenants/{tenant}/nodes/{node}/registrations`: the registration of a Node of another Deployment pins its endpoint and Deployment Key by fetching `/.well-known/aidash` and checking the key ID typed from the other side, and a Peer Mapping then binds each sending subject to a local credential. Every Federation request carries a token signed with the sending Deployment's Deployment Key, so no shared secret is configured. Set `[deployment.key] file` in each federating Deployment's settings file. Create the key with `manage deployment-key generate`. The environment variables above do not set it. `[deployment] federation = false` refuses Federation with other Deployments. In each Deployment's Tenant, register a research agent and approve it on each Node, because a Catalog Approval applies to one Node. Scoped discovery exchanges metadata over the federation API; no remote database access is needed. Each workspace retains an authoritative home Node.

`manage server` runs the API, outbox publisher and domain-event consumer. All roles publish durable Run activations, while only `manage worker` and `manage serve` pull from the shared activation consumer. Workers have four execution slots by default (`AIDASH_WORKER_SLOTS`, 1–4) without the management HTTP listener. Set `AIDASH_PROBE_LISTEN` to enable the separate health probe listener. `manage serve` runs both roles. Provision the activation stream once with `aidash activation-provision` using NATS operator credentials and the same Node/namespace settings; runtime credentials do not create it by default. Broker outages retain database recovery. See [worker activation operations](docs/operations/worker-activation.md) for permissions, deadlines, diagnostics and verification. To exercise recovery, stop a **worker** process while leaving its server, PostgreSQL and NATS running, then restart it with the same configuration. The lease expires after 30 seconds. Task and run IDs remain stable.

## Tools and coordination

Every Agent retains `workspace_read` and `human_request`. The other Built-in tools are included by default and can be removed through `remove_default`. Cluster coordinators must retain `task_create`, `task_delegate` and `agent_discover`. The model's final text completes its task and publishes a final artifact. A coordinator must wait for its subtasks and synthesize their artifacts. Unresolved children require an explicit, audited abandonment before the parent can finish.

Independent reads in one model response can run concurrently when both the Agent's `tool_parallelism` and the process's `AIDASH_TOOL_PARALLELISM` exceed one. See [parallel read batches](docs/features/parallel-read-batches.md).

An Agent can require Checks, such as a fixed test command, to pass on the exact files a Run produced before the Run may complete; the model's own success claims never count. See [Completion Gate](docs/features/completion-gate.md).

Tools are immutable Registry descriptors with provider-owned behavior and a JSON Schema for arguments. For an HTTP integration, register this configuration on its exact Node:

```json
{
  "registry_deployment": "aidash://node-a",
  "provider": "integration.http@1",
  "operation": "invoke",
  "default_alias": "research_lookup",
  "tier": "integration",
  "transport": {
    "transport": "http",
    "endpoint": "https://tools.example.com/research",
    "replay": "unsafe"
  }
}
```

HTTP and MCP tool transports never name a Deployment secret: `credential_env` is rejected on registration, import and installation, and a stored Tool that still carries it is refused when it runs (#296). Tenant-owned secrets for tools are planned in the same Issue.

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
        "registry_deployment": "aidash://node-a",
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

The alias stays stable across versions. Run admission saves the complete dependency closure; subsequent steps use that snapshot while rechecking current resource authority. Memory and reference context require explicit Memory or Source bindings. Sandboxed tools, including shell and Python, are built-ins that each Tenant approves before use, and the Deployment offers them only when it runs their provider. See the [Registry capability contract](docs/operations/registry-capabilities.md) for approval, lifecycle operations and the drained upgrade procedure.

MCP and Agent integrations use `integration.mcp@1` and `integration.agent@1` with their configuration under `transport`. An Agent integration creates and delegates a child of the invoking task. HTTP tools propagate `Idempotency-Key`, but HTTP/MCP behavior remains `Unsafe` without a verified provider contract: an uncertain effect pauses for reconciliation and is never automatically repeated. Transport replay labels do not weaken that rule. Native echo and native HTTP fetching are unsupported for ordinary new registrations.

## History compaction

Context compaction uses a Rust reimplementation of [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction). Jev runs through OpenRouter's System One API (`POST https://openrouter.ai/api/v1/systemone`) with the Tenant's OpenRouter Provider Credential and `provider.zdr = true`; on Cloud the Credential Broker relays it under a `decision` Capability Token (ADR 0041). A Decider version may list later Routes, such as Cloudflare's Clef through OpenRouter's Decisions API (`POST https://openrouter.ai/api/alpha/decisions`) on the hosts the Route names; a later Route is asked only when every earlier one is unavailable (ADR 0066). No TypeSafe account or environment key is involved, and the calls are billed to the Tenant's OpenRouter account. A Run is pruned only through the compaction Decider its Agent's Binding (or, in a generated chain, its generation policy) names, once that exact Decider version has a Catalog Approval on the Node; a Run without one is not pruned. Jev is contacted only when the model's context budget is exceeded and old tool pairs can be pruned; those requests send a classification view of the history to OpenRouter and TypeSafe, within the byte budget the Decider version declares.

Jev decides whether to keep each old tool call and its full result. Aidash keeps both, keeps the call with a shortened result, or removes the pair. It preserves the first and latest six history events (Model Turns on the `native` Projection Version) and all human/non-tool events. Optional task/workspace/memory snapshot material is separately bounded to the model window with an explicit truncation marker. Workspace observations contain paged summaries and record IDs; agents use `workspace_read` for full details. Legacy observation results are converted to this view on replay, while durable audit records remain unchanged. Every original history event stays in the Run's Context Journal, whatever the model-visible history keeps. Without a `context_policy`, an Agent never summarizes history. When every Route of a Decider is unavailable, the step is retried after 2, 4, 8, 16 and 32 seconds before the Run pauses. A Decider that cannot run pauses the Run with `decider_unapproved`, `decider_calls_exhausted` or `decider_unavailable`, and insufficient reduction without a Summary Stage pauses it with `context_unreducible`; neither changes the saved context. See the [compaction contract](docs/protocol.md#context-compaction) for input fitting and request limits. Protocol tests use a local Jev fixture; live service access and decision quality are not established by those tests.

An Agent version can opt into `"context_policy": {"version": "context-recovery/1", ...}`. If pruning cannot fit the request, an explicitly approved summarizer model can merge older tool history, continuing from the pruned history, into a structured Execution Summary; the summarizer has no fallback and never stands in for a Decider that cannot run. A provider-reported Context Overflow re-runs this pipeline a bounded number of times with a smaller effective window. Output truncation and refusals pause the Run with a typed reason, and their tool calls are never dispatched. See [context recovery](docs/protocol.md#context-recovery) for the fields, approval and adoption rules, and metrics.

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

Trunk's `trunk-fmt-pre-commit` action formats staged files on every commit. Trunk installs the Git hook the first time it runs in a checkout. Run `trunk git-hooks sync` once in a new clone or worktree before your first commit. The hooks live in the repository's shared Git directory, so every worktree uses them.

PostgreSQL integration tests use `aidash-orm-test-postgres:17-pg-jsonschema-0.3.4`,
built by `scripts/build-test-postgres.sh` from the Dockerfile's `test` target.
The multi-node Compose fixture uses `aidash-compose-postgres:17-pg-jsonschema-0.3.4`
from its `runtime` target with separate database initialization. The distinct tags
keep Compose rebuilds from replacing the image used by isolated ORM fixtures.

`check.sh` runs Rust unit and PostgreSQL integration tests, builds the dashboard, and executes the two-node acceptance scenario starting from a real Chromium dashboard. It also verifies remote human controls, all four tool transports, and the browser scenarios. The scenario starts real Aidash processes, PostgreSQL and NATS with a deterministic OpenRouter-compatible protocol fixture. The same checks and Trunk lint run in GitHub Actions. Both nodes and workers start with NATS unavailable; the scenario verifies queued events drain after the broker connection is restored. It kills Node B's worker after an external effect but before its result is persisted, restarts the worker, and checks the same run completes with no duplicate effect. Reports are written to `private/acceptance/report.json`.

Golden Path also requires subject-scoped remote execution on these real Node processes. Separate source and receiver tenants, credentials, catalog approvals and Peer Mappings authorize the work. It kills the receiver Worker after a Home message or task completion commits but before its response is delivered, checks recovery of the same admission and Run without duplicate effects, and repeats recovery with a peer outage and with a grant revoked while the Worker is stopped. Cross-tenant/workspace access, disabled Agents, unauthorized dependencies and legacy admission are rejected. The `scoped_remote_execution` report records each scenario, the Worker process IDs and persistent execution IDs without credentials. A successful legacy scenario alone cannot pass this gate.

Use `scripts/test-acceptance.sh` to build the current checkout's binary and frontend before running the full gate. The report includes the Git revision, dirty status, source fingerprint and binary digest; changing the source or binary while the gate runs fails verification. The browser authentication fixture exercises real Bearer-authenticated APIs; it does not establish an OIDC provider login flow.

The script's default run keeps only the browser specs tagged `@golden-path`, which need the state its scenarios create. `scripts/test-acceptance.sh --browser-suite` runs every other browser spec after the Node setup alone, so a Golden Path failure does not hide the browser suite. CI runs the two modes as separate jobs.

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

## Native memory

The Rust Hindsight provider stores memory units, facts and graph edges in PostgreSQL with pgvector and PGroonga. Qdrant is not used. Retrieval combines semantic, keyword, graph and temporal rankings; a reranker is a required policy role (a registered model or the explicit `Rrf` passthrough). Agents bind the immutable Memory provider and workspace Sources through Binding contract 1. Recovery journals and visibility checks retain the admitted provider identities across restarts.

An authorized manual retry renews a native-memory operation's retry ledger while preserving its cumulative call, token and cost totals. Exhausting any of those durable budgets pauses local and remote Runs with `durable_memory_budget` without failing their Tasks. Recovery offers only a new follow-up Task, whose Run receives a new operation; retrying the unchanged Binding cannot restore the spent budget.

Three contracts own different parts of this behavior: Run-context continuity and compaction (#113, #116), persistent source ingestion and semantic retrieval (#110), and extraction, review and consolidation of reusable knowledge (#118, #141). See [architecture](docs/architecture.md#memory-contracts), the [traceability matrix](docs/memory-traceability.md) for the status of each requirement, [memory evaluation](docs/memory-evaluation.md) and [memory recovery](docs/memory-recovery.md).

## Database migrations

Reinhardt owns the app migration graph under `server/migrations/<app_label>/`. The native baseline supports empty PostgreSQL databases and replay of its own history; existing SeaORM or experimental migration databases are not adopted. Run `cargo run --locked -p aidash-server --bin manage -- migrate` through the native management CLI. See the [migration correspondence and maintenance cutover procedure](server/migrations/README.md) before switching deployments.

The PostgreSQL image supplies the `pg_jsonschema` library. Native history creates and owns an absent extension. For a database-scoped, non-superuser application role, an administrator provisions the extension first; rollback preserves that borrowed extension. Use an empty database template. The Compose `local-dev-db` preparation command applies this same history and validates the extension version before the backend starts.

### Database roles

Aidash uses two PostgreSQL logins, so that row-level security can bind the server (#223):

- `aidash_owner` runs migrations and owns every table. Only `aidash migrate` and other one-off maintenance commands use it.
- `aidash_server` is the login of servers and workers. It is `NOSUPERUSER NOBYPASSRLS` and holds `SELECT`, `INSERT`, `UPDATE` and `DELETE` on the owner's tables and use of its sequences, through default privileges. It never receives `TRUNCATE`, which skips row-level security.

Servers and workers (`serve`, `server`, `worker`, `runserver` and `runworker`) refuse to start when their login is a superuser, has `BYPASSRLS` or owns any database object, and they no longer apply migrations: they start only once `aidash migrate` has applied every migration. A PostgreSQL superuser provisions the roles once: it creates them, makes `aidash_owner` own the database, creates the `pg_jsonschema`, `vector` and `pgroonga` extensions, and grants the default privileges. The bundled PostgreSQL image does this on a new data directory when `AIDASH_OWNER_PASSWORD` and `AIDASH_SERVER_PASSWORD` are set, as Compose and the local Kubernetes manifests do; `deploy/postgres/init-roles.sh` lists the statements for other servers. A Compose volume created before these roles must be recreated with `docker compose down -v`.

## CI and coverage

Application PRs run **CI**. Merges into `develop/0.1.0` and other `develop/**`
branches run **Release CI**, which reuses the same validation suite and supplies
the successful source evidence for staging and Release PR automation. Main
pushes retain CI's release verification.

CI runs Trunk, Clippy, eight Rust test partitions, a separate coverage upload, Bruno API contracts, infrastructure checks, collaboration browser tests, and the PostgreSQL/NATS/Chromium acceptance suite in separate jobs. Each Cargo test target belongs to exactly one partition; the inventory is derived from locked Cargo metadata and validated before execution. Dependency caches are isolated by partition. All eight LCOV reports are required and uploaded together under the same Rust coverage flag. `CI Success` requires every CI job to succeed, including the Codecov upload. Use that check for branch protection. Rust coverage uses `cargo llvm-cov` with real PostgreSQL tests and uploads an explicit LCOV file through Codecov OIDC. Codecov measures `crates/` and `server/src/`; tests, migration plumbing and generated API files are excluded. Browser tests establish dashboard behavior and are not included in the Rust coverage percentage.

Cluster recovery, transaction cluster recovery, and isolated capability runtime acceptance are available for manual execution. Run `bash scripts/test-cluster.sh kubernetes` after installing the browser dependencies; use the `transactions` or `remote-memory` profile for those recovery suites. Run `scripts/test-capability-cluster.sh` for isolated capability acceptance. These suites are outside the CI workflow and its `CI Success` prerequisites.

The coverage uploader uses a pinned Codecov CLI from PyPI; see the [download outage and recovery condition](docs/operations/ci-coverage.md).

Run `scripts/test-rust.sh --partition identity` for one partition, or `python3 scripts/rust-test-partitions.py --check` to inspect the complete inventory. Run `scripts/test-rust.sh --coverage` to produce `coverage/rust.lcov` locally (requires `cargo-llvm-cov` 0.8.7 and `llvm-tools-preview`). `scripts/check.sh` runs the full local suite. Cargo and npm lockfiles remain tracked for reproducible dependency resolution.

Run `npm exec --yes --package=@usebruno/cli@3.1.3 -- scripts/test-bruno-api.sh` with the test PostgreSQL and NATS services running to verify the real HTTP API. The [Bruno collection](server/tests/bruno/README.md) checks all 281 native endpoints with 3–10 scenarios each, including scoped authorization, input rejection, state changes, browser cookies/CSRF, finite SSE replay and frontend caching against a disposable database and the compiled server. Sanitized reports include the source revision and executable hash.

Third-party attribution for adapted code is in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

HTTP request logging, admission limits, SSE capacity, and the optional Prometheus
listener are documented in [HTTP protection and observability](docs/operations/http-observability.md).

## Desktop client

The [Tauri 2 desktop client](desktop/README.md) bundles the shared dashboard and connects to an existing local or remote Aidash server. It supports external-browser Google sign-in and persistent OS-protected credentials.

## License

The licensing of Aidash is being prepared in [#124](https://github.com/kent8192/aidash/issues/124). Until it takes effect, the [interim notice](LICENSE) reserves all rights in original Aidash material, except `aidash-capability` (Apache-2.0). Third-party material keeps its own terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). External code contributions are not accepted until then, and issues are welcome.

## Deployment Marketplace Connections

Registry imports immutable plugin copies from GitHub or a configured standalone
Marketplace. An import URI is `marketplace://<plugin-uuid>/<semver>`; it never
supplies an endpoint, Library owner or credential. Configure an optional
`[marketplace]` endpoint and one `[[marketplace.connections]]` per local Tenant:

```toml
[marketplace]
endpoint = "https://marketplace.example.com"
poll_seconds = 60

[[marketplace.connections]]
tenant = "brave-otter-12345678"
connection_id = "00000000-0000-0000-0000-000000000001"
owner_account_id = "00000000-0000-0000-0000-000000000002"
owner_tenant_id = "brave-otter-12345678"
secret_file = "secrets/marketplace-connection"
```

The Marketplace must authorize the Connection and its enrolled Library owner.
For a personal Library, omit `owner_tenant_id` and provide
`personal_identity_id`, the retained local External Identity UUID whose Personal
Tenant receives the copy. The default exception also requires an online shared
authentication binding proving that Identity belongs to the Connection owner
Account; a configured local UUID alone is insufficient. Independent Deployment
sign-in cannot infer this association from email. Importing that personal Library
without that proof or into any other Tenant
requires an explicit `marketplace.personal_library.import` policy; the default
Tenant administrator wildcard alone does not grant it. Local `registry.create`
is checked before fetching and again before storing.

The client refuses redirects and validates the returned plugin/version binding,
canonical file paths, base64, size limits, tree digest and Agent Plugin schema.
Registry retains the Tenant-local copy and its authoritative acquisition expiry.
Expired acquisitions refuse new Agent versions and Runs; an authorized renewed
acquisition can update the receipt without replacing the pinned bytes or source.
Configured Connections synchronize Recalls periodically. Learned Recalls survive
Marketplace outages and Connection removal and refuse new Runs of that plugin
version; they do not cancel admitted Runs or delete local copies. An offline
Deployment learns newly issued Recalls only after synchronization. Normal local
execution does not contact the Marketplace. These supported-runtime checks do
not prevent the person running a Self-hosted Deployment from modifying its runtime or copying already
delivered content; this is not a DRM guarantee.
