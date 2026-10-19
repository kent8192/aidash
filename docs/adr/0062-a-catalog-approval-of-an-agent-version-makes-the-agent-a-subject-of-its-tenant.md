# A Catalog Approval of an agent version makes the agent a subject of its Tenant

Every execution decision evaluates each agent in a Run's chain as a subject of the Tenant's
policy, and an agent missing from it is refused. Only generation created agent subjects, so even
after Tenants own their Registry and their Catalog Approvals (ADR 0029), a Tenant could not run an
agent it had written and approved until a Deployment Administrator replaced its whole policy
(#355). We decided that approving an agent version for use also creates that version's agent
subject, in the same transaction, so that one approval makes the agent runnable. This mirrors
generation, which registers, approves and adds its subject together.

The new subject joins `members` when the Tenant's policy marks it Assignable, and no group
otherwise. The approver may name other Assignable Groups only when it also holds
`subject.group.update`, so an Assignable Group stays the only authority a Tenant Administrator
can confer. The subject has no delegator: its authority is its groups, intersected with the
Run's chain.

## Considered Options

- **A separate action, such as `subject.create` for agents.** It would keep the policy out of the
  catalog, but every agent would take a second step, and only Tenant Administrators could take
  it. Rejected.
- **Implicit subjects.** Policy evaluation would treat every approved agent version as a subject
  without storing it. That puts an exception into the evaluator and leaves the policy bundle
  incomplete as the record of who may act. Rejected.
- **The approver as delegator,** as generation makes its requester. The agent would stop for
  everyone when its approver leaves the Tenant or is disabled, though the approval is the
  Tenant's decision and not a personal delegation. Rejected.
- **Inheriting the groups of the previous version.** Without an owner per definition, anyone
  holding `registry.create` can register the next version, which would then silently receive
  what an administrator gave an earlier one. Rejected; the dashboard only pre-fills them.

## Consequences

- A Catalog Approval of an agent version advances the Tenant's policy revision, so it can
  conflict with a Deployment Administrator's policy replacement that expects an older revision.
- Each version is its own subject, because agent subject identifiers carry the version.
- `subject.group.update` also moves agent subjects between Assignable Groups.
- Withdrawing an approval stops the agent but leaves its subject. Approval creates a subject only
  when none exists, so approving again never re-enables a disabled one.
- An agent subject never counts as a Tenant Administrator, since it has no Mapping (ADR 0026).
- Generated agents keep their requester as delegator, and their groups and roles stay as their
  generation specification names them.

## Amendment (2026-10-18, #357)

A Tenant Administrator may confer any authority that concerns its Tenant by writing the Tenant's
policy (#357), so an Assignable Group is no longer the only authority a Tenant Administrator can
confer. It stays the only one an approver confers through a Catalog Approval: the subject joins
`members`, or other Assignable Groups when the approver also holds `subject.group.update`. No
Tenant policy confers authority that concerns the Deployment.
