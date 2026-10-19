# A Tenant keeps at least one administrator, judged conservatively

Supersedes ADR 0021's consequence that Aidash does not count remaining
administrators.

With self-service Tenants on Cloud, an orphaned Tenant becomes a support case
for Aidash staff, who are Cloud's only Deployment Administrators. Deny rules and conditions
still make an exact count impossible. Aidash therefore judges conservatively.
Before a member leaves, a Mapping is disabled, Assignable Group memberships
change, or a Tenant Administrator changes policy, the same transaction checks
whether at least one other enabled Mapping remains whose subject the policy
allows the administration actions without conditions. If none remains, Aidash
refuses the change with 409. A conditional allow does not count, so any error
refuses a change that might have been safe and never allows one that orphans
the Tenant.

## Considered Options

- **Do not count (ADR 0021).** This leaves orphaned Tenants to Deployment Administrators.
  Rejected.
- **Require a handover before leaving.** Disabling a Mapping or changing groups
  could still orphan the Tenant. Rejected.
- **Schedule orphaned Tenants for deletion.** This destroys data because of an
  administrative mistake. Rejected.

## Consequences

- The last administrator leaves by deleting the Tenant or by first making
  someone else an administrator.
- A Deployment Administrator replacing the whole policy is exempt; that is a deliberate
  deployment-wide action.

## Amendment (2026-10-18, #357)

Tenant Administrators may now replace their Tenant's whole policy (ADR 0021, amended).
The administration actions the check requires therefore gain `policy.read` and
`policy.update`, so that a Tenant always keeps someone who can repair its policy without
a Deployment Administrator. A Tenant Administrator's replacement runs the check on the
new policy with no Mapping excluded: the actor's own Mapping counts. Actions that a
policy writer can restore, such as `credential.*`, `peer_mapping.*` and `decision.read`,
are not required.
