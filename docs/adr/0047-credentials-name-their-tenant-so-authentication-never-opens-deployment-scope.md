# Credentials name their Tenant, so authentication never opens Deployment Scope

Decided on 2026-10-15 (#263). Every transaction opens with a tenancy scope (ADR
0040), but authenticating a request reads Tenant-owned rows before the Tenant is
known: a Subject Credential is found by its digest, and a dashboard request's
Mapping by its id. Opening Deployment Scope for those lookups would make the
exception the most frequent kind of transaction, turn its `application_name`
audit into noise, and let a bug in sign-in code read every Tenant's rows.

Instead, what a caller presents names its Tenant. A Subject Credential is
`aidash_subject_<Tenant name>_<secret>`, and the dashboard's context selector is
`mapping:<Tenant name>:<id>`. The lookup runs in that Tenant's scope, where a
wrong Tenant name finds nothing. Tenant names reveal nothing about their Tenant
and are never reused, so carrying one discloses nothing and can never come to
mean a different Tenant.

## Considered Options

- **Deployment Scope with an authentication reason on every request.** The
  smallest change. Rejected for the reasons above.
- **Classify a credential index (digest to Tenant) as Deployment-wide.** It
  copies Tenant-owned data out of the Tenant and out of Tenant Deletion's purge.
  Rejected.

## Consequences

- Sessions, desktop tokens and Deployment Administrator API tokens are
  Deployment-wide, so their lookups open the no-Tenant scope. A Deployment
  Administrator acting on one Tenant works in that Tenant's scope. When the
  route names only a record, a read-only Deployment Scope finds the record's
  Tenant first (amended on 2026-10-16: today's routes name the record, not the
  Tenant).
- The only cross-Tenant step left is listing the Tenants an External Identity is
  mapped into, at sign-in, when switching Tenants and when it asks to join one.
  It opens a Deployment Scope. Asking to join writes a Deployment-wide
  registration row, so the reason allows writes (amended on 2026-10-16).
- Inbound federation resolves its Tenant through a read-only Deployment Scope
  until #241 gives every Node a Tenant; from then on the receiving Node names it.
- The secret and the Mapping id have fixed lengths, so the Tenant name is parsed
  from the right; today's Tenant identifiers may contain `_` and `:`.
- Subject Credentials issued before this change stop working. Aidash is
  undeployed, so no migration is provided.
