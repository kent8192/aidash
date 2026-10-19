# Every Tenant has a root record that outlives its purge

Decided on 2026-10-15 (#238). A Tenant existed only as the key of its policy
bundle, so nothing could hold its label or who created it and when, and nothing
stopped a Tenant name from being reused. ADR 0040 required a root record that
survives purge and left the choice of record to #238 and #239. A new `tenants`
record is that root: its Tenant name is the key, and it holds the Tenant label,
the creation time, the creating principal and whether it is a Personal Tenant.
Every Tenant-owned `tenant` column references it, and the policy bundle is an
ordinary Tenant-owned record. Purge clears only the label. The name, creation
time and creating principal stay as the tombstone, so retained audit history
stays valid. Because the record is never deleted, its key alone keeps names from
being reused.

## Considered Options

- **Delete the root at purge and keep only a separate name reservation.** That
  gives two records whose lifetimes must match, and it loses who created a
  Tenant whose audit history is still retained. Rejected.
- **Extend the policy bundle into the root.** This conflates the Tenant with one
  of its revisable policy documents. ADR 0040 had already decided that the
  bundle stops being the root. Rejected.

## Consequences

- A Deployment Administrator can no longer create a Tenant implicitly by
  replacing a policy at revision 0. A policy replace for a Tenant without a root
  record fails, and every Tenant is created through the Tenant creation API,
  under a name Aidash assigns.
- The root record is Deployment-wide, not Tenant-owned. It outlives the
  Tenant's data, and the Tenant switcher, Tenant-creation limits, the Deployment
  Administrators' Tenant list and the Invitation landing page all read it across
  Tenants without opening Deployment Scope.
- Tenant-creation limits count root records by their creating principal.
