# Deployment Administrators do not run work outside Tenants

A Deployment Administrator's bearer token could create workspaces and tasks and start Runs
that belonged to no Tenant ("legacy Deployment Administrator workspaces"). Once every Tenant
owns its Registry (ADR 0029), such a Run has no Registry in which to resolve its
agent, model, tools and skills. Rather than keep a Deployment-level Registry
partition for them, Issue #243 removes untenanted execution: every workspace,
task and Run belongs to a Tenant, and a Deployment Administrator who wants to run work does so
inside a Tenant, through a Mapping or a Subject Credential, like anyone else.

## Considered Options

- **A Deployment Administrator-only Registry partition.** A reserved tenant value would keep
  legacy Deployment Administrator workspaces working with the least change, but it would
  reintroduce a Registry outside every Tenant and a second execution model.
  Rejected.
- **Assign Deployment Administrator workspaces to one configured Tenant.** Deployment Administrator actions
  would then write into that Tenant's content, which conflicts with Deployment Administrators
  having no routes into Tenant content. Rejected.

## Consequences

- Deployment administration (policies, Mappings, credentials, peers, Deployment
  settings and the Marketplace) is unchanged; only Deployment Administrator execution is removed.
- Admission, workers and the APIs that create workspaces, tasks and Runs always
  have a Tenant, and the paths for Runs without an authorization record go away.
- Tests that relied on the Deployment Administrator token for execution act as a mapped subject
  of a Tenant instead.

## Amendment (2026-10-17, #243)

- The legacy peer protocol (`/federation/v0.1/offers`, `/workspace` and `/control`) and
  `/api/remote` are removed together with Deployment Administrator execution, because they
  admit work only into workspaces that belong to no Tenant. Scoped remote delegation, through
  durable source grants and receiver admissions, remains, and peer configuration is unchanged.

## Amendment (2026-10-27, #485)

- A Deployment Administrator's one remaining control over a Run is the content-free Safety Stop (pause or cancel
  with a reason, never resume), recorded in ADR 0101. The ordinary `/api/runs/{id}/control` route refuses every
  Deployment Administrator.
