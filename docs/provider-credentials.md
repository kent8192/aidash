# Provider Credentials

Provider Credentials are Tenant-owned metadata records obtained only through
Provider Authorization, the provider's OAuth flow, as decided in ADR 0006.
Key Material passes from the server-side exchange into the write-only Provider
Credential Store. Aidash exposes no HTTP create/rotate endpoint or dashboard
form accepting typed or pasted provider keys. OAuth connection and reconnect
flows are tracked in [#158](https://github.com/kent8192/aidash/issues/158).
The v1 Provider Catalog contains `openrouter`, with base URL
`https://openrouter.ai/api/v1`.
Registry model and embedding configurations use `provider_credential =
"openrouter"`, mutually exclusive with `credential_env`; they never contain a
Provider Credential ID. The removal of operator `credential_env` keys and the
mandatory Store requirement for self-hosted nodes belong to #158's migration.

Configure the optional server settings only for a dedicated BYOK project:

```toml
[provider_credentials.store]
byok_project_id = "your-aidash-byok-project"
environment_id = "develop"
fingerprint_env = "AIDASH_SECRET_PROVIDER_FINGERPRINT"
max_per_tenant = 20
```

Supply an independent random fingerprint root key of at least 32 bytes through
the named environment variable. Fingerprints use independent derived Tenant
keys, HMAC-SHA256, and the first eight bytes in hexadecimal. They are metadata,
not bearer values. Key Material is not written to PostgreSQL, events, responses,
request logs, or browser persistence.

GCP deployment renders this non-secret Store descriptor from the
`aidash-provider-credentials` VM metadata into a read-only Reinhardt settings
source for migrations and the server. BYOK-enabled runtime secrets must contain
the stable `AIDASH_SECRET_PROVIDER_FINGERPRINT` value; a missing or short value
blocks startup. The key is never generated or changed during deployment. When
BYOK is omitted, the managed Store remains unset and no fingerprint is required.

Tenant metadata endpoints are GET `/api/tenants/{tenant}/provider-credentials`
and GET `/api/tenants/{tenant}/provider-credentials/{id}`. Revoke and delete remain
available, alongside `/api/tenants/{tenant}/provider-credential-bindings`.
Lifecycle operations and
binding updates require the current `expected_revision`; stale writes return 409. Sending `provider_credential_id: null` explicitly unbinds a provider and
retains its revision history; unbind before deleting the last bound record.
Public policy actions are `provider_credential.read`, `.revoke`, `.delete`, and
`provider_credential_binding.read`, `.update`. Internal Provider Authorization
writes retain the `.create` and `.rotate` policy decisions. Resources
carry the provider attribute and UUID or Provider Catalog ID respectively.
Authenticated Subjects may operate only within their Tenant; authenticated
operators retain explicit policy decision audits. Responses use
`Cache-Control: no-store`.

Tenant settings show metadata, binding selection, revocation and deletion without
key-entry controls. This example uses test fixture metadata:

![Provider Credential metadata and binding management](images/provider-credentials.png)

The application-layer `Service::create` and `Service::rotate` use cases remain
internal write paths for #158's server-side OAuth callback. Native management
composition accepts typed providers and `SecretString` directly and runs policy
decisions; there is no Key Material HTTP serializer, SDK operation, environment
variable, or settings input for this write path. Tests seed through it directly.
Creation persists `pending` before provider verification and secret storage.
Rotation pins a verified new version before disabling the old one. If that
post-commit disable fails, it returns the committed metadata with a cleanup
warning; the PostgreSQL active-row inventory retains the reconciliation work.
The supervised reconciler retries unpinned versions. With background tasks
disabled, cleanup remains pending until reconciliation resumes. Revocation
commits its irreversible metadata state and audit before disabling all versions,
so effective access closes immediately. A failed database commit leaves external
versions unchanged; interrupted or failed disables are retried from revoked rows.
The API returns the committed revoked state while cleanup is pending. Deletion
refuses a bound record, then commits a `deleted` tombstone before destroying any
versions or deleting the secret. The private version pin marks unfinished
cleanup until the idempotent external effects complete; clearing it records a
metadata-only cleanup event without changing the deletion revision. Interrupted
cleanup is retried from that tombstone and never restores effective access.
Subject writes preserve their committed result if the separate authorization
audit cannot finalize, and report that audit failure through static telemetry.
Authorized mutation attempts finalize their allow audit independently of the
operation result, including failed creates that leave pending or deleted metadata;
the original operation error is preserved. Cleanup completion events use the same
Tenant and Provider Credential read-policy checks in state, polling, and SSE.
A supervised reconciler scans
expired PostgreSQL pending records, disables unpinned active versions left
by interrupted rotations, disables all versions of revoked records, and completes
deleted tombstones with unfinished cleanup. Each supervised pass processes at most 25 eligible
metadata records in UUID order, retains its cursor across passes, and waits 60
seconds after completing a page. A failed candidate is retried on the next sweep
without blocking later Tenants. It never lists Secret Manager secrets.

Admission records a local-only Provider Credential ID per Run and provider.
This includes the enabled workspace semantic index's embedding provider, even
when the Agent's Model uses an environment source. Missing embedding bindings
therefore reject admission before the Run is committed. RequiredHome remote
semantic bindings reject Tenant-backed workspace embeddings before binding or
dispatch: this path has no approved local Run pin or BYOK maintenance authority.
Environment-backed remote embeddings remain supported.
Calls check that record's current active state and current version pin; changing
a binding cannot retarget admitted Runs. Receiving federation admission uses
the mapped local Tenant. The configured Credential Broker handles plaintext
reads and provider routing after metadata validation. Without that broker,
Tenant access fails with `credential broker not configured` and never falls
back to environment keys.

Explicitly authorized local maintenance without a Run resolves the current
Tenant binding and includes its memory indexing, retention, reflection or
retrieval purpose in the access context. Run calls keep their admission pins.
Both paths check current metadata before issuing a broker capability token.

Workbench tests currently support environment-backed Models only. A Model using
Tenant Provider Credentials is rejected before a sandbox session is admitted;
Workbench has no Run or memory-maintenance authority for Tenant provider access.

Terraform requires an existing billing-enabled `byok_project_id` distinct from
the application project. This project contains only Provider Credential
secrets. Runtime creation permission exists only there; management is limited
to `aidash-<environment_id>-cred-` by a project-number-based IAM condition. The
runtime identity has no BYOK `versions.access` or `setIamPolicy`. Bootstrap
configures DATA_READ and DATA_WRITE audits and the fixed custom roles once.
Deploy can grant only the fixed runtime Create/Manage roles and cannot edit
BYOK roles. Human-run bootstrap creates the broker identities listed in
`byok_broker_environments` and binds the fixed BrokerRead role to each
environment's Secret name prefix. Only these broker bindings grant BYOK
payload access; runtime and deployment identities have no payload read grant.
Human-run bootstrap also binds deploy to a prefix-conditioned delete-only
`aidashByokRetire` role and an unconditioned project list-only
`aidashByokRetireInventory` role. Deployment cannot grant either role through
its unchanged Create/Manage allowlist. Retirement inventories only metadata,
deletes the exact environment prefix, and confirms it is empty before disk,
runtime identity and IAM removal. Existing running writers are sealed first;
stopped or missing VMs are never awakened. Cleanup failures retain the disk and
identity for an idempotent retry. Residual: project-level listing makes BYOK
secret names visible to deploy; no payload or version read permission is added.

Outputs `byok_project_id` and `secret_prefix` supply the secret namespace. The
existing shared-project deployment identity could manage a broker service
account's IAM policy there, and a broker deployment identity could execute code
with Key Material access. The deploy pipeline remains part of the trust root;
the IAM deny policy and entry-point hardening are tracked in
[#151](https://github.com/kent8192/aidash/issues/151). A separate credential
enclave is deferred. See the residual deployment trust boundary in
[the infrastructure guide](../infra/gcp/README.md).
