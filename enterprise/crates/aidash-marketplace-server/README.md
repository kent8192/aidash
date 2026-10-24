# Standalone Marketplace

This executable serves the Marketplace independently of Cloud. It has no
`aidash-server`, Harness, Runtime or Cloud-schema dependency. The separately built
`enterprise/marketplace-web` app talks to this backend through the same origin.
A reverse proxy terminates TLS and routes `/api`, `/auth` and `/healthz` here.
Internal billing/enrollment/Recall endpoints must not be exposed by the public proxy.

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
