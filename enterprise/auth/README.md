# Standalone Authentication

This is one full-stack Reinhardt Pages project at `enterprise/auth`. Its
layout follows the pinned CLI output from:

```sh
reinhardt-admin startproject auth --with-pages
cd auth
reinhardt-admin startapp authentication --with-pages
```

The reference scaffolds use templates from Reinhardt revision
`b6a006a769ca340c22a51e67fef1bb4be1dc3039`. The package remains a named member
of the Enterprise workspace and retains its service/protocol/migration identity.

```text
src/
  apps.rs
  apps/authentication.rs
  apps/authentication/
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
OIDC and private gRPC/mTLS retain their external contracts. Product access/refresh tokens remain native. The existing
Claude-authored CSS and Pages design are preserved. Components use `page!`.
The app's `use_authentication` hook owns state, clears proof fields between
steps, and closes the GCIP client on lifecycle cleanup. Product tokens remain
on the native side of the typed server-function boundary.

## Build and browser verification

Install Rust 1.96+, `wasm32-unknown-unknown`, wasm-pack 0.15, protoc, Python 3,
Docker and Chrome/Chromium. From this directory:

```sh
bash scripts/wasm-build-dev.sh
# Use scripts/wasm-build-release.sh for a release bundle.
cargo clippy --locked -p aidash-auth-server --features browser-e2e --all-targets -- -D warnings
cargo clippy --locked -p aidash-auth-server --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Native and WASM use this single Cargo package and `enterprise/Cargo.lock`.
Generated bindings go to `dist-wasm/`; Python assembles bindings and `static/`
into `dist/`. The product image serves that bundle on the same origin as its
typed server functions and protocol endpoints. No Node.js, npm, Vite,
TypeScript build or Playwright configuration is required.

From the repository root, build both products before Auth browser E2E, which
covers the cross-product BFF flow:

```sh
bash enterprise/auth/scripts/wasm-build-dev.sh
bash enterprise/marketplace/scripts/wasm-build-dev.sh
driver="$(python3 scripts/install-standalone-webdriver.py)"
python3 scripts/test-standalone-browser.py --product auth --driver "$driver"
docker build -f enterprise/auth/Dockerfile -t aidash-auth:local .
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

## Provision and run

Use `bootstrap.sql` once as a PostgreSQL administrator. Assign strong, distinct
LOGIN credentials through the secret manager and do not grant either role
membership in Cloud or Marketplace roles. Both roles use the `aidash_auth` schema;
the migration owner can change its schema and the application role has DML only.
The schema must already exist before the process connects.

Copy `auth.example.toml` into an administratively provisioned configuration location. Mount
database URLs, client/resource secrets, the RSA signing key, the base64-encoded
32-byte token-encryption key and mTLS files at the referenced paths. The Firebase
browser API key is public configuration, not a replacement for client secrets.
Only the `cloud` and `marketplace` clients are accepted initially, with exact
registered HTTPS callbacks.

```sh
# Use the migration owner's database URL file for this command.
cargo run --locked --manifest-path enterprise/Cargo.toml -p aidash-auth-server -- \
  --config /run/aidash/auth-owner.toml migrate
# As owner, run seal-ledger.sql after migrating/provisioning.
# Use an otherwise equivalent config with the application-role URL to serve.
cargo run --locked --manifest-path enterprise/Cargo.toml -p aidash-auth-server -- \
  --config /run/aidash/auth-runtime.toml serve
```

`migrate` applies this service's Reinhardt migration set and provisions the
declared clients and signing key. `serve` requires current migrations; it does
not create the schema or run Cloud migrations. Build and provide this project's
`enterprise/auth/dist` as `assets_dir`, or set `AIDASH_AUTH_ASSETS_DIR`.
`AIDASH_AUTH_CONFIG` can supply the configuration path instead of `--config`.
The product image bundles its own Pages assets at `/app/pages`.

Place public HTTP behind the configured HTTPS origin. Forwarded HTTPS metadata
is accepted only from the proxies in `trusted_proxy_ips`, each an exact IP or a CIDR
network such as `"10.44.0.0/14"` (host bits must be zero; every peer in the network is
trusted, so restrict who can reach the service with the standalone chart's `networkPolicy`; an invalid entry fails startup), and every response outside loopback
development carries `Strict-Transport-Security`. Keep gRPC private and require
client certificates. `/healthz`, OIDC discovery and JWKS support service checks;
user interactions, accounts and protected data retain their own authentication
and CSRF checks. Real GCIP project/provider configuration and workload access to
the GCIP account-status API are deployment prerequisites.

## Identity migration

Export legacy Identity records through an authorized migration procedure,
preserving their UUID, issuer and subject. Import the administratively provisioned JSON file:

```sh
cargo run --locked --manifest-path enterprise/Cargo.toml -p aidash-auth-server -- \
  --config /run/aidash/auth-owner.toml import-legacy /run/aidash/identities.json
```

The input is an array of `aidash_domain::accounts::ExternalIdentity` records.
An authorized offline export of Cloud's `dashboard_identities` maps `id`,
`issuer`, `subject` and `last_valid_at` without changing them. Set `status` to
`Disabled` and `disable_epoch` to 1 when `disabled_at` is present; otherwise use
`Enabled` and 0. Always set `first_account` to null and `linked` to false:

```json
[
  {
    "id": "00000000-0000-0000-0000-000000000001",
    "issuer": "https://securetoken.google.com/example-project/tenants/shared",
    "subject": "original-gcip-subject",
    "first_account": null,
    "linked": false,
    "status": "Enabled",
    "disable_epoch": 0,
    "last_valid_at": null
  }
]
```

Exclude email and display attributes. The serving Authentication Server never
receives Cloud database credentials. Before cutover compare the export's count
and sorted UUID/issuer/subject tuples with the source snapshot, and verify the
same tuples after import. Import refuses a source pair already stored under a
different UUID. Replaying an export preserves later Links and stop observations.
It must not assign Accounts or active login Links. Existing Account association
history cannot be changed by import. Stage this before enabling Cloud's shared
authentication mode: old interactive sessions will require login, but already
admitted work must be verifiable by its original source even before first
Account assignment. Do not replace local Mapping, Billing ownership or Run
origin references with Account IDs.

Keep the direct GCIP and common-auth sign-in settings mutually exclusive. A
rollback of product images must keep common authentication and its current stop
state authoritative; disabling it to bypass the authority is not a valid
rollback. Retain Authentication Server records and Account/source stop epochs,
including across restoration of product data. Back up each independently
provisioned service database under the deployment's recovery procedure before
cutover; this change does not run a production migration or backup.

At first new sign-in a never-assigned Identity chooses a new Account or attaches
to an existing Account after fresh proof on both sides. Equal emails do not link.
An Identity's first Account remains historical after unlinking, and reattachment
to another Account is not supported. Source-session revocation does not itself
cancel admitted work.

Verified upstream email and display name travel with that original source's
proof and sessions so existing Cloud display and Demo Reservation behavior can
continue. They are ephemeral issuer metadata, expire with their proof/session,
and are never combined into an Account profile or used to link Accounts. The
service's migration set includes `0002_ephemeral_source_profiles`; migrate before
rolling the serving image. Product-local display erasure remains authoritative.

## Authority and revocation

Cloud's service URI SAN grants product validity/revocation operations, not common
Account administration. Authentication Administrators use a distinct mTLS peer
identity. Source restoration has its own explicit product permission and still
requires fresh successful upstream status; a product must check its Deployment
Administrator authorization before requesting it.

The service distinguishes an interactive Product Session from durable work's
original source. Ordinary logout ends a product/device session, while explicit
all-products logout ends all sessions. Neither ends admitted durable work.
Source disablement, common Account Suspension and withdrawn local authority block
the next relevant admission. Retained stop generations prevent a temporary
outage followed by stop/restoration from silently auto-resuming old work.

Fresh GCIP status is usable for five minutes. A lookup failure has grace only
until fifteen minutes from the last success. Failure to reach this Authentication
Server itself immediately refuses new protected operations. Restoring an Account
or source does not revive its old sessions or automatically resume old Runs.

Browser sessions use twelve-hour absolute and thirty-minute idle limits. A
refresh cannot extend their absolute limit. Refresh families are scoped by
product/device; the HTTP wrapper uses a durable request ID and encrypted bounded
response cache to distinguish a lost response from inconsistent reuse. A crash
between protocol rotation and response-cache persistence can still require a
new login; it never falls back to an active-token cache or revives stopped state.

See [deployment and certificate rotation](../deploy/standalone-service/README.md)
and the [approved design](../../docs/authentication-and-standalone-marketplace-design.md).

## Upstream browser interop

Only the GCIP SDK bridge uses handwritten browser ESM. Its fixed Firebase App
and Auth modules are served locally from `static/assets/vendor`, with license
notices and recorded upstream/vendored SHA-256 digests. The assembler verifies
those digests. No CDN fetch is needed at build or runtime to load the SDK code;
live provider/authentication API access remains necessary. Upstream credentials
stay transient, SDK persistence stays in memory, and clients close between fresh
proof steps. Fonts are static IBM Plex files with their SIL Open Font License.
