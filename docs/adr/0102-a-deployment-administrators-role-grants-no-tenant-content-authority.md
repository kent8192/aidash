---
status: accepted
---

# A Deployment Administrator's role grants no Tenant content authority

A Deployment Administrator reads or changes a Tenant's content and settings only through that Tenant's authority: its own Mapping selected in the browser, or a Subject Credential the operation supports, decided by the Tenant's current policy. Neither a browser Deployment Administrator context nor the shared recovery API token reaches Tenant collections, streams, settings, Provider Credentials, Agent Generation, transactions, Workbench incidents or Tenant audit. The single retained exception is content-free pause/cancel of a known Run executed by this Deployment through the audited Run management contract (#485). Administrative authorities explicitly retained by #363, #238 and #239 (Tenant creation, Registration decisions, Invitations, Mappings, whole-policy replacement and the policy read it needs, deletion/restore/purge) are unchanged. This implements #301 (#484); ADR 0034, 0043 and 0046 applied the same principle to narrower areas.

A refused administrator request returns 403 with no Tenant content; routes that already answer a uniform 404 keep it, and a deleted Tenant still answers 410.

## Considered Options

- **Keep administrator read access for support.** Rejected: a Deployment-wide role would override every Tenant's policy, and support can act through an explicit, auditable Mapping.
- **Let a held grant silently fall back to administrator authority when no Mapping is selected.** Rejected: the authority used must always be the one the caller explicitly selected.
- **Answer every refusal with 404.** Rejected: administrators create Tenants, so hiding existence protects nothing and obscures the reason.

## Consequences

- A Deployment Administrator can still obtain a Mapping into any Tenant by deciding a Registration Request (#363). That path is visible and audited but not closed here; it is tracked in #561.
- The dashboard hides Tenant content views and Tenant pickers in the Deployment Administrator context and directs the person to switch to a Mapping.
- Ordinary `/api/runs/{id}/control` refuses Deployment Administrators, including shared-bearer resume, under #485's Safety Stop contract (ADR 0101).
- Routes deny a Deployment Administrator by default; only routes that serve a retained authority admit one. A new route is Tenant-only unless it is deliberately marked otherwise.
- Headless setups issue a Tenant's first Subject Credential with a host-side `manage` command. Host access already reaches the database, so this opens no new route; over HTTP, issuance still needs Tenant authority (ADR 0063).
- Workbench adoption, which let an administrator place a registered agent into a Tenant's Creator, is removed rather than moved: it was an administrator-only Tenant mutation.
