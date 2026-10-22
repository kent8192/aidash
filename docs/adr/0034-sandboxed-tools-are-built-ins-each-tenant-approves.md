# Sandboxed tools are built-ins that each Tenant must approve

Sandboxed tools (formerly the Host tier) reached a Tenant only as packages an
Deployment Administrator staged for it, through `/api/marketplace/host-packages` or the
`default_host_packages` setting, and that a Deployment Administrator then approved. With
Tenant-owned Registries, Deployment Administrators have no routes into a Tenant's content, so Sandboxed
and research packages become built-in definitions offered by the Deployment.
Unlike other built-ins, each needs an explicit Catalog Approval in the Tenant
before use, because they act outside Aidash. The Deployment's runner sets their
limits, and a Deployment without a runner offers none.

## Considered Options

- **Deployment Administrators keep staging packages per Tenant.** This keeps Deployment Administrators on the
  path of every Tenant, including Personal Tenants. Rejected.
- **Deployment Administrators publish them to the Marketplace and Tenants install them.** The
  copies would drift from the runner they depend on, and installation adds
  nothing when only the Deployment can run them. Rejected.

## Consequences

- `/api/marketplace/host-packages` and `default_host_packages` are removed.
- `task_assign` leaves the tier and becomes an ordinary built-in, because it
  only starts a generation inside Aidash.
- A new Tenant cannot use any Sandboxed tool until it approves one, so a
  Personal Tenant's first use of shell or code takes one approval step.

## Amendment (2026-10-17, #311)

- Research providers, built in or declared (ADR 0048, ADR 0055), need a Catalog Approval like Sandboxed tools, though they do not run in an Execution Pod. Each provider's research operations, including the host-supplied `find`, are approved as one group.
