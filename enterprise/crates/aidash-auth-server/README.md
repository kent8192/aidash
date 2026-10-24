# Shared Authentication Server

This process owns human authentication for Cloud and the standalone Marketplace.
GCIP remains the upstream verifier. The service issues first-party OAuth/OIDC
credentials; Cloud consumes private validity and lifecycle RPCs through
`reinhardt-grpc` with mTLS. It does not start the Cloud runtime, workers or NATS,
and it does not provision Cloud Tenants.

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
not create the schema or run Cloud migrations. Provide the separately built
`enterprise/auth-web/dist` as `assets_dir`, or set `AIDASH_AUTH_ASSETS_DIR`.
`AIDASH_AUTH_CONFIG` can supply the configuration path instead of `--config`.
The container image bundles the independent auth web build at `/app/auth-web`.

Place public HTTP behind the configured HTTPS origin. Forwarded HTTPS metadata
is accepted only from explicitly trusted proxy IPs. Keep gRPC private and require
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

See [deployment and certificate rotation](../../deploy/standalone-service/README.md)
and the [approved design](../../../docs/authentication-and-standalone-marketplace-design.md).
