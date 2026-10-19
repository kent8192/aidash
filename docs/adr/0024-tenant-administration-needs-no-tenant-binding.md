# Tenant Administration needs no Tenant Binding; Invitations address requests to a Tenant

Supersedes the first consequence of ADR 0021.

Personal Tenants from direct Google sign-in and self-service Tenants have no
Tenant Binding, so ADR 0021's rule left them without an administrator, and with
no way to add members except a Deployment Administrator. A Tenant Administrator now exists in
any Tenant whose policy permits the administration actions.

A generic OIDC Registration Request still carries no Tenant, so it stays
Deployment Administrator-approved. A request reaches a Tenant Administrator only when it is
addressed to that Tenant: through a Tenant Binding (GCIP) or through an
Invitation the Tenant issued. The requester chose that Tenant, which is the
basis for showing it their Display Attributes.

## Considered Options

- **Show unbound Tenant Administrators every untargeted request.** This would
  expose every applicant's Display Attributes to every Tenant. Rejected.
- **Joining immediately on opening an Invitation link.** Anyone holding a
  leaked link could join. Rejected in favor of a single-use link followed by an
  approval.
- **Email-addressed invitations.** Emails never link identities or grant
  authority in Aidash. Rejected.

## Consequences

- Without a Tenant Binding or Invitation, a Tenant Administrator sees only the
  External Identities already mapped into its Tenant.
- Assignable Groups, the ban on acting on one's own Mapping or subject, policy
  revisions and the append-only administration history apply unchanged.
- A Tenant created through Aidash makes its Tenant Creator its first
  administrator. Its initial policy permits every action within the Tenant, as
  a personal Tenant's does.
