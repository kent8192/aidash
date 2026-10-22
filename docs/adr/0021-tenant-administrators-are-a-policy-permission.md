# Tenant Administrators are a policy permission, bounded by Assignable Groups

A Tenant Administrator is not a grant record like the Deployment Administrator grant. It is an External Identity mapped to a subject that its Tenant's policy permits to perform `registration.read`, `registration.approve`, `registration.reject`, `mapping.read`, `mapping.disable` and `subject.group.update`. We chose this over a tenant-scoped copy of the Deployment Administrator grant (#144) for three reasons:

- Issue #144 forbids a second account system beside External Identities, Mappings and Deployment Administrator grants.
- Mapped subjects already manage parts of their own Tenant through policy actions (`generation.approve`, `provider_credential.*`).
- Granting and revoking the authority then lands in the existing policy revision history.

Approvals need a user subject, and today only a Deployment Administrator can create one by replacing the whole policy. A Tenant Administrator therefore needs a narrow way to write policy. Unrestricted policy edits would let it grant itself anything, so its writes are bounded instead:

- It can create a user subject only while approving a Registration Request.
- It can place user subjects only in Assignable Groups, which are groups a Deployment Administrator has marked as assignable.

## Considered Options

- **Tenant-scoped Deployment Administrator grant** `(identity, tenant)`: a second authorization system outside the policy, with its own audit trail and revocation path. Rejected.
- **Tenant Administrators edit the whole policy**: lets a Tenant Administrator grant itself any action. Rejected.
- **Deployment Administrators keep creating subjects**: leaves Aidash staff on the path of every new user, which was the problem #144 exists to remove. Rejected.

## Consequences

- Tenant Administrators exist only where a Tenant Binding ties Registration Requests to a Tenant (GCIP). With generic OIDC, Registration Requests carry no Tenant, so only Deployment Administrators approve them.
- Whether Tenant Administrators can appoint each other is a per-Tenant Deployment Administrator choice: whether the administrators' group is marked assignable.
- A Tenant Administrator can approve only into a subject it creates during that approval, or re-approve an External Identity into a subject that identity was previously mapped to. Without this rule, it could hand out a privileged subject that sits outside the Assignable Group boundary.
- A Tenant Administrator cannot act on its own Mapping or subject. Aidash does not count how many administrators remain, because deny rules and conditions in the policy make that count unreliable. If a Tenant loses all its administrators, a Deployment Administrator recovers it.
- Within its Tenant, a Tenant Administrator can disable any other Mapping and remove any user subject from an Assignable Group. That risks locking people out of the Tenant, but it cannot grant anyone more authority. Lockouts are traced through the audit history and repaired by a Deployment Administrator.
- Subjects created and group changes made by a Tenant Administrator advance the policy revision under that identity as the actor. A Deployment Administrator editing an older revision gets a conflict and must reload, as already happens after a generation is published.
- Approvals, rejections and Mapping disables are recorded in an append-only history with the acting External Identity and its context (Deployment Administrator, or a Mapping and its Tenant).
- Deployment Administrator authority and the Deployment Administrator approval flow do not change.

## Status

Amended 2026-10-15 (#238): a Tenant created through Aidash starts with an Assignable `administrators` group that grants every action, so its Tenant Administrators can appoint one another without a Deployment Administrator. That is the default; a Deployment Administrator can still unmark the group for a Tenant. Superseded in part by ADR 0024 (no Tenant Binding needed) and ADR 0026 (the last administrator is protected).

Amended 2026-10-18 (#357): a Tenant Administrator may confer any authority that concerns its Tenant, so the rejection of "Tenant Administrators edit the whole policy" no longer holds. A subject that its Tenant's policy permits to write the policy may replace it, under the revision check and the last-administrator check (ADR 0026). A Tenant's policy cannot confer authority that concerns the Deployment; that stays with Deployment Administrators, Tenancy Rules and Deployment settings. Assignable Groups still bound `subject.group.update` and approvals. Marking a group assignable becomes a policy write, so whether Tenant Administrators appoint one another is the Tenant's choice. A subject permitted to replace the policy holds every authority within its Tenant, so the rule that a Tenant Administrator cannot act on its own Mapping or subject does not limit what its replacement may change, agent subjects included; a Catalog Approval stays the way an agent subject is normally created (ADR 0062).
