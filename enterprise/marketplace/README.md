# Standalone Marketplace

This is one full-stack Reinhardt Pages project at `enterprise/marketplace`. Its
layout follows the pinned CLI output from:

```sh
reinhardt-admin startproject marketplace --with-pages
cd marketplace
reinhardt-admin startapp distribution --with-pages
```

The reference scaffolds use templates from Reinhardt revision
`b6a006a769ca340c22a51e67fef1bb4be1dc3039`. The package remains a named member
of the Enterprise workspace and retains its service/protocol/migration identity.

```text
src/
  apps.rs
  apps/distribution.rs
  apps/distribution/
    client.rs       client/components/    # declarative page! views
                    client/hooks.rs        # mounted state and lifecycle hooks
    server.rs       server/               # protocols, stores, migrations, tests
    server_fn.rs    server_fn/            # shared typed RPC declarations
    services.rs     services/             # target-specific services
    serializers.rs                        # shared wire data
    urls.rs         urls/                 # app-owned client/server routers
  client.rs         client/lib.rs         # inventory-driven WASM launcher
  config.rs         config/               # settings, apps and route composition
  bin/manage.rs                         # native migrate/serve commands
```

Pages calls the app's typed `#[server_fn]` stubs. Native registration injects the
owning service and request; adapters retain the existing cookie, CSRF and
admission policies without a network hop or a second database connection.
OIDC/BFF, Registry and binary download APIs retain their external contracts. Product access/refresh tokens remain native. The existing
Claude-authored CSS and Pages design are preserved. Components use `page!`
and the owning app's hooks. Marketplace publication forms use `form!`,
`use_form` and `use_form_action` for typed values, validation and pending/error
state; protected reads use scoped `use_resource` without retaining stale success.

## Build and browser verification

Install Rust 1.96+, `wasm32-unknown-unknown`, wasm-pack 0.15, protoc, Python 3,
Docker and Chrome/Chromium. From this directory:

```sh
bash scripts/wasm-build-dev.sh
# Use scripts/wasm-build-release.sh for a release bundle.
cargo clippy --locked -p aidash-marketplace-server --features browser-e2e --all-targets -- -D warnings
cargo clippy --locked -p aidash-marketplace-server --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Native and WASM use this single Cargo package and `enterprise/Cargo.lock`.
Generated bindings go to `dist-wasm/`; Python assembles bindings and `static/`
into `dist/`. The product image serves that bundle on the same origin as its
typed server functions and protocol endpoints. No Node.js, npm, Vite,
TypeScript build or Playwright configuration is required.

From the repository root, build and test Marketplace independently. The Auth
browser suite additionally covers the cross-product BFF flow:

```sh
bash enterprise/marketplace/scripts/wasm-build-dev.sh
driver="$(python3 scripts/install-standalone-webdriver.py)"
python3 scripts/test-standalone-browser.py --product marketplace --driver "$driver"
docker build -f enterprise/marketplace/Dockerfile -t aidash-marketplace:local .
```

The driver installer writes only task-local `private/` files and matches an
existing Chrome installation. `WEBDRIVER_URL` can select an existing WebDriver
instead. Rust tests use `reinhardt::test::fixtures::wasm::e2e`, including
`browser_client` and `BrowserClient`; ordinary unit/PostgreSQL runs omit the
`browser-e2e` feature. Native management commands remain specialized for
explicit owner migration and runtime serving.

Browser fixtures exercise real Pages, typed RPC, OIDC/BFF and PostgreSQL service lifecycles.
Native transport/PostgreSQL tests enforce authorization, schema ownership and
session contracts. Live GCIP, a real cluster, payment integration and production
cutover require separate acceptance and are not established by these fixtures.

Set `AIDASH_MARKETPLACE_ASSETS_DIR` to `enterprise/marketplace/dist` when running
the native executable locally; the image sets it to `/app/pages`. The public
edge routes the product origin to this one service and denies `/internal/`.

## Database and startup

A PostgreSQL administrator provisions `bootstrap.sql` once. Enable LOGIN and
supply distinct strong credentials through your secret manager. Neither role
may be a member of a Cloud/Auth role; grant neither service access to this schema.
The owner applies native Reinhardt migrations; the runtime role has DML only.
Each role's `search_path` must begin with `marketplace`. The migration ledger
is sealed against writes by non-owner roles after migration.

```sh
AIDASH_MARKETPLACE_DATABASE_URL="$MARKETPLACE_OWNER_URL" \
  cargo run --manifest-path enterprise/Cargo.toml -p aidash-marketplace-server -- migrate
# Export the variables in .env.example with runtime-role credentials.
cargo run --manifest-path enterprise/Cargo.toml -p aidash-marketplace-server
```

No Cloud migrations, NATS, worker process or Tenant provisioning is involved.
Secrets are read from mounted files, each at least 32 characters. Register the
`marketplace` confidential OIDC client with the Authentication Server using the
exact `${AIDASH_MARKETPLACE_ORIGIN}/auth/callback` redirect and the same client
secret. Register `marketplace-resource` separately for introspection and read-only
common Account status checks. It has no Account administration permission. HTTPS is required;
HTTP is accepted only for loopback development URLs.

With an HTTPS origin the service itself refuses plain-HTTP requests (except
`GET /healthz` and the three `/internal/` POST routes `/internal/billing/events`,
`/internal/enrollments` and `/internal/recalls`, which are machine-to-machine and
Bearer-authenticated, and which the public edge denies) and sends `Strict-Transport-Security`. Forwarded HTTPS metadata is
accepted only from the proxies listed in the comma-separated
`AIDASH_MARKETPLACE_TRUSTED_PROXY_IPS`, each an exact IP or a CIDR network such as
`10.44.0.0/14` (host bits must be zero; every peer in the network is trusted, so
restrict who can reach the service with the standalone chart's `networkPolicy`). An invalid entry fails startup. Leave it unset only for loopback HTTP
origins; with an HTTPS origin and no trusted proxy, every request except
`GET /healthz` and the three `/internal/` POST routes is refused with 400.

The BFF verifies signed OIDC issuer/audience/nonce and PKCE. It stores access and
refresh tokens in Marketplace persistence and returns only an opaque HttpOnly
cookie. Each protected request performs remote introspection; an unavailable
Authentication Server refuses admission. Rotation uses a durable random request
ID and a per-session PostgreSQL lock, so replicas cannot race refresh or resurrect
a logged-out session. Sessions expire after 12 hours absolute or 30 minutes idle.
The service does not share another product's cookie.

## Distribution

Public listing/detail metadata includes Publisher, provenance URL, license, USD
price and Declared Permissions. Only published versions appear in public lists.
A personal Publisher belongs to the common Aidash Account. Free acquisition
creates an authoritative persisted personal Entitlement. The browser cannot
claim a paid purchase; paid acquisition requires the trusted Billing integration.

Publish a JSON map of relative paths to standard RFC 4648 base64 file contents.
It must contain root `plugin.json`, for example:

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "my-plugin"
}
```

The enclosing version request is `{ "version": "1.0.0", "files": {...} }`.
The envelope version must be a semantic version. A manifest may omit its optional
version; when present it must exactly match the immutable envelope.
`expected_sha256` is optional and, when present, must match the existing Agent
Plugin tree digest (`sha256:` prefix). The server validates the actual Agent
Plugin format, path traversal, canonical base64, at most 512 files and at most
8 MiB decoded content. Versions are immutable; an existing version returns 409.
The HTTP JSON body limit is 12 MiB. Artifacts remain in this service's own database.
There are no unauthenticated file/storage URLs that bypass Entitlement checks.

An authenticated personal Library download is available at
`GET /api/marketplace/plugins/{uuid}/versions/{version}/download`.
Registry Connections instead use `POST /api/marketplace/downloads` with the
Connection credential in `Authorization: Bearer ...` and this body:

```json
{
  "connection_id": "UUID",
  "owner_account_id": "UUID",
  "plugin_id": "UUID",
  "version": "1.0.0"
}
```

Optional `owner_tenant_id` selects only the Tenant bound to that Connection.
Tenant Entitlements are keyed by **both Deployment and Tenant**, preventing a
same-named Tenant on a different Deployment from inheriting ownership. The
Account must match the explicit Account Link made when creating the Connection;
caller-supplied ownership assertions cannot expand it. New acquisitions recheck
Account status with the Authentication Server. Download responses include the
authoritative `entitlement_expires_at` when rights have a deadline; Registry must
retain and enforce that deadline for new registration/execution. Installed copies need no
Marketplace request for each execution. Local Catalog Approval and offline
Recall enforcement belong to Registry/Run admission.

`GET /api/marketplace/recalls` with the same credential returns the complete
append-only Recall set. The authorized Aidash Recall integration may issue a Recall through
`POST /internal/recalls` with `{plugin_id,version,reason}` and the independent
Recall Bearer secret. Publisher ownership and Authentication Administrator status
do not grant this Marketplace operation.
This prevents new Marketplace acquisition; already learned Recall records must
remain locally even after Connection removal. Local copies offer no DRM guarantee
against a Self-hosted runtime changed by whoever runs its machines. Strict metering needs a remote
MCP/API implementation of the protected operation.

## Service integrations

Provision Connection enrollment through `POST /internal/enrollments` using the
trusted enrollment-service Bearer credential and `{deployment_id,tenant_id?}`. Give its
ten-minute, one-use enrollment token to the Deployment Administrator. The signed-in person
explicitly links their Account by creating a Connection with
`POST /api/marketplace/connections`, `{deployment_id,enrollment_token}` plus
`Origin` and `X-Marketplace-CSRF` from `/api/marketplace/session`. The credential
is returned once; only its SHA-256 digest is stored. Never distribute the global
enrollment secret to a Self-hosted installation. Revocation is product-local.

The existing Billing service sends verified/idempotent grant or revoke events to
`POST /internal/billing/events`, authenticated by the independent billing Bearer
secret. Body: `{event_id,plugin_id,owner,expires_at,revoked}`. `owner` is either
`{"kind":"account","id":"UUID"}` or
`{"kind":"tenant","id":{"deployment_id":"...","tenant_id":"..."}}`.
An exact event retry is accepted; changed data with the same event ID returns 409. Existing Billing ownership references are not rewritten. The secret is
intended for an authenticated Billing adapter after provider verification,
never a browser. Whoever maintains the Billing integration must provision and
activate the live payment-provider adapter separately.

## Verification

The fixture starts an isolated PostgreSQL 17 Docker container and provisions its
own distinct migration/runtime roles. Docker must be available:

```sh
cargo test --manifest-path enterprise/Cargo.toml -p aidash-marketplace-server --lib
```

The test exercises real HTTP requests, Reinhardt native migrations and ORM
persistence using the runtime role. It protects acquisition ownership, immutable
versions, lifecycle admission and durable Recalls; the authentication status
fixture only represents the external introspection dependency.
