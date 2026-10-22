# Tenant purge follows the table classification and keeps audit history for a finite period

Decided on 2026-10-17 (#239). After a Tenant Deletion's grace period, its data
is purged, but its audit history must outlive that purge, and some Tenant data
lives outside PostgreSQL where deleting rows alone would orphan it for good: one
Secret Manager secret per Provider Credential on Cloud, reachable only through
`provider_credentials.secret_resource`, and capability objects on a volume,
reachable only through `core_objects`.

Purge is a resumable, idempotent job on the hourly retention loop, under its own
read-write Deployment Scope reason. It first revokes and deletes Provider
Credentials and their external secrets, then removes capability object files,
then deletes every Tenant-owned row the checked-in table classification (ADR 0040) lists, except audit history, and finally clears the root record's label
(ADR 0050) and marks the deletion record purged. A test fails when a Tenant-owned
table is neither purged nor declared audit history.

Audit history is the administration history, authorization decisions, policy
revisions, catalog history, and #225's Evidence Revocation and Retention Rule
history. It is kept for a finite Deployment setting after purge (one year by
default) and then deleted by the same loop. Who may read it after purge is
#301's decision.

## Considered Options

- **Walk parent tables from the Tenant now.** Most Tenant-owned tables do not
  carry `tenant` until #265, so the walk would be a second, hand-kept list.
  Rejected: purge lands after #265.
- **Keep audit history forever.** Personal data in it would never leave.
  Rejected.
- **Delete rows first and sweep external stores for orphans.** The secrets carry
  no Tenant label and the reconciler never lists them, so nothing could find
  them again. Rejected.

## Consequences

- A Deployment Administrator may purge a deleted Tenant at once, skipping the
  grace period; Tenant Administrators may not.
- A purge that keeps failing retries from its last step and is shown to
  Deployment Administrators as stalled. A purge may lag its deadline by up to an
  hour.
- Out of reach, and documented as such: the runner journal (no Tenant key, and
  acknowledgement blanks it), platform and broker logs, provider-side prompt
  caches, and OpenRouter keys, which people delete at OpenRouter; the deletion
  dialog says so.
- Tenant-creation limits free a slot when the deletion record is marked purged.
