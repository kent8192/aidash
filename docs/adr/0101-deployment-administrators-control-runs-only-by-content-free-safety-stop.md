---
status: accepted
---

# Deployment Administrators control Runs only by a content-free Safety Stop

A Deployment Administrator has no authority over a Tenant's content (ADR 0043), yet must be able to stop a
Run that spends money or does harm, for example a `run_id` found in a log. Until now the general Run control
route accepted a Deployment Administrator's resume as well as pause and cancel, returned the content-bearing
`RunInspection`, and wrote `run.control` without saying who acted. The management route returned recovery
diagnostics and could be replayed without a receipt.

A **Safety Stop** is a Deployment Administrator's pause or cancellation of one Run this Deployment executes,
named by Tenant and Run, always with a reason. It never resumes a Run and never shows Run content. It is
carried by the existing `GET`/`POST /api/runs/{id}/management` route. Issue #485 delivers it together with the
refusal of every ordinary Deployment Administrator `/api/runs/{id}/control` request (the shared-token and
browser alike, before any mutation or serialization), so no intermediate release keeps resume as an
administrator fallback.

## Decision

- **Naming the target.** The expected Tenant is a `tenant` query parameter on `GET` and a body field on
  `POST`. The Run's Tenant is looked up once, read-only, under the `RunTenant` Deployment Scope and compared
  with the named one; a missing Run and another Tenant's Run give the identical 404. All other work runs in the
  named Tenant's scope. A deleted Tenant is refused with 410, retries included, because the command takes the
  Tenant's shared deletion lock, which Tenant Deletion takes exclusively.
- **What an administrator reads.** Only the Run ID, typed phase, typed control, the control revision and the
  control source, through a dedicated projection that never reuses `RunManagement` or `RunInspection`.
- **Control revision.** `runs.control_revision` changes only when control changes (Tenant pause or resume, a
  Safety Stop, automatic pause or resume), never with worker progress, so a command's expected revision is not
  made stale by unrelated work. A database trigger bumps it for any writer of `control` that does not bump it
  itself, so a future writer cannot forget.
- **Control source.** `runs.control_source` records the kind of actor that last changed control: Tenant,
  Deployment Administrator or system. Anyone who can see the Run sees it (never the person, never the reason),
  so the dashboard can say "Paused by a Deployment Administrator". A Safety Stop pause clears the automatic
  identity-status marker, so automatic recovery never resumes it; only fresh Tenant authority resumes.
- **Command.** `pause` or `cancel`, the expected control revision, an `Idempotency-Key` UUID header and a
  required reason of 1 to 1000 trimmed characters (the Evidence Revocation rules). A terminal Run, or one whose
  cancellation is already requested, returns a no-op receipt whatever the revision. Otherwise a revision
  mismatch is 409. Pausing an already paused Run is accepted: control stays paused, the source becomes
  Deployment Administrator and the revision increases.
- **One table for audit and receipt.** `run_safety_stops` holds Tenant, Run ID, action, actor kind, browser
  Identity ID, reason, timestamp, operation ID, the input fingerprint and the outcome. The operation ID is
  scoped by Tenant and caller (every holder of the shared token is one caller; a named administrator is
  another). An identical retry returns the stored receipt with no second effect, even after a Tenant resumed the
  Run; the same operation ID with different input is 409. The control change, this row and the `run.control`
  event commit in one transaction; a failure to write the row rolls everything back, and workers are notified
  only after the commit. Refused requests (403, 409, 410) are not recorded.
- **Lifetime.** The table is audit history: it has no foreign key to Runs or Workspaces, is kept after a Tenant
  purge for `audit_retention_days` and then erased. The Tenant reads it under `decision.read`; a Deployment
  Administrator's receipt grants no read of it, and it is closed while the Tenant is deleted or purged.
- **Required-Home receivers.** A receiver Run whose content is hidden is still stoppable by this Deployment;
  the command is never forwarded to another Deployment, and the reason is never sent. Existing remote state
  synchronization carries the phase and control.
- **Subjects.** A Tenant Subject's contract on the route is unchanged; a Subject request carrying an
  administrator-only field is refused with 400. Subject operations are not added to the Safety Stop audit.

## What the receipt promises

A receipt acknowledges a durable request. Pause prevents later scheduling and does not interrupt an inference in
progress. Cancel asks the existing mechanisms to interrupt current work and cannot undo a call already sent to
an external system. Neither is a promise that execution has stopped, and no cancellation latency is guaranteed.

## Considered Options

- **Keep `/control` for administrators and add audit.** Rejected: it returns content-bearing `RunInspection`
  and allows resume, which is Tenant authority.
- **Separate tables for audit and for receipts.** Rejected: two writes that must agree, with the same lifetime
  and the same key.
- **Compare the Run's `revision` as the expected version.** Rejected: it moves with every worker step, so a
  safety command would fail exactly when the Run is busy.
- **An administrator release or resume operation.** Rejected: resumption needs fresh Tenant authority.

## Consequences

- ADR 0043 stands: a Deployment Administrator still runs no work and reads no content. The Safety Stop is its
  one retained exception on a Run, recorded here.
- Existing Runs start at control revision 0 with the Tenant as the source.
- Logs that name a `run_id` also carry the Tenant where it is at hand, so an administrator can name the Tenant
  of a runaway Run.
