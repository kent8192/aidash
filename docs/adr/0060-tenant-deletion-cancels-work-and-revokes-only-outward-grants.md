# Tenant Deletion cancels work and revokes only outward grants

Decided on 2026-10-17 (#239). Recording a Tenant Deletion makes the Tenant
unusable at once, and a restore within the grace period (30 days by default, a
finite Deployment setting) must bring it back intact. Revocation is
irreversible, so "credentials are revoked" and "restore brings it back intact"
cannot both hold for every credential. Aidash therefore splits what deletion
does by what a restore can bring back.

- Running and queued work is **cancelled** with the reason `tenant_deleted`, not
  paused. A restore brings back data and configuration, never work.
- **Plan Connections** are revoked at OpenAI and deleted at once (ADR 0007): they
  are one subject's grant, and every owner reconnects after a restore.
- **Pending Invitations** are revoked at once: they are links already handed
  out, and a restore does not revive them.
- **Subject Credentials** and **Provider Credentials** are only refused while the
  Tenant is deleted. A restore brings them back; purge revokes and deletes the
  Provider Credentials before anything else.

Restore is decided by `tenant.delete` under the Tenant's own policy, frozen
since deletion, for a subject whose Mapping and External Identity are enabled at
restore time, so whoever may delete may undo it. A Tenant a Deployment
Administrator deleted is restored only by a Deployment Administrator, so a
Tenant cannot reverse a Deployment-level measure. `tenant.delete` joins the
actions the last-administrator guard (ADR 0026) counts, so a member never leaves
behind a Tenant nobody can delete.

## Considered Options

- **Pause work and resume it on restore.** Plan Connections are already gone
  after deletion, so many resumed Runs would fail for lack of credentials, and a
  Run paused for 30 days resumes against a changed world. Rejected.
- **Revoke every credential at deletion.** Safest, but every restore would
  require reconnecting every provider and reissuing every Subject Credential,
  which defeats a grace period meant for mistakes. Rejected.
- **Only Deployment Administrators restore.** Every restore on Cloud would be a
  support case for Aidash staff. Rejected.
- **Only the deleting administrator restores.** Another administrator could not
  undo a colleague's mistake. Rejected.

## Consequences

- The deletion record is Deployment-wide, one row per deletion, so it outlives
  purge and is readable while the Tenant's scope refuses everyone. Deleting a
  restored Tenant again starts a new grace period.
- A browser Mapping of a deleted Tenant receives 410 `tenant_deleted`; Subject
  Credentials and desktop tokens receive the same 401 as for an unknown Tenant,
  so outsiders cannot probe whether a Tenant was deleted.
- Restore cannot use ordinary authentication, which refuses the deleted Tenant.
  Its path only uses the caller's Mapping in the Tenant being restored and that
  Tenant's own policy, in that Tenant's scope.
- Two administrators may alternately delete and restore; each step is recorded,
  and Aidash does not guard against it.
- Amended 2026-10-17 (PR #351): during the grace period the stopped Runs are
  recorded as paused with the reason `tenant deleted`, because a cancelled Run
  is reclaimed by workers to finish it, which the deleted Tenant's authority
  refuses. A restore turns them into cancelled Runs; nothing resumes them.
