# Registries belong to Tenants; there is no shared Registry

The Registry was shared by every Tenant of a Deployment. Deployment Administrators managed it,
and Catalog Approvals decided what each Tenant could use. This kept personal and
self-service Tenants from creating anything, and it raised the questions of
owning shared IDs, references to other Tenants' definitions, and idempotency
keys shared across Tenants.

Each Tenant now has its own Registry. A Marketplace package is installed into a
Tenant as that Tenant's own copy. Only built-in definitions are common to a
Deployment, and they are read-only. Deployment Administrators decide what the Marketplace
offers, not what each Tenant holds. Definitions in a Tenant's Registry are made
available to its Nodes and subjects, and they run inside a Node.

## Considered Options

- **A shared Registry plus Tenant-owned definitions.** This needs an ownership
  table, a check of every reference for visibility, and Tenant-scoped
  idempotency over a global key. Rejected for its permanent complexity.

## Consequences

- Registry identifiers are unique within a Tenant, and versions remain
  immutable within it.
- Catalog Approval no longer means a Deployment Administrator's approval for a Tenant. It is a
  Tenant Administrator's decision that one exact version in the Tenant's
  Registry may be used on one Node, optionally only by certain subjects there.
