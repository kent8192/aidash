# Production updates take a Pre-update Backup that excludes the Home recovery ledger, and it is the only supported database restore

Decided on 2026-10-20 in the #435 grilling. A production update seals the Environment (admission closed, writers drained, no active work), then takes a Pre-update Backup under a dedicated Backup for GKE plan before Terraform or Helm changes anything. If the backup fails or does not finish in time, the update fails and unseals before it changes anything, and it waits for an explicit retry. Every backup plan selects one ProtectedApplication, `aidash`, whose label selector includes every resource in `aidash-production` except the Home recovery ledger claim, labelled `aidash.run/backup: exclude`. The in-place restore plan selects the same application. The only supported database restore is a Pre-update Backup of an update that failed before any writer started again. Because nothing was written after the seal, that restore loses no acknowledged write and leaves the ledger consistent. A restore from the daily backup rewinds the database, which needs the continuity design in #433 first, so it is a documented last resort.

## Considered Options

- **A manual backup under the daily plan.** Rejected: it shares the daily plan's 30-day retention and its freshness checks and alerts.
- **`pg_dump` of PostgreSQL only.** Rejected: it misses NATS JetStream, the Runner journal and the capability objects, and its run time grows with the database.
- **A namespace-scoped backup, with the ledger excluded only at restore time.** Rejected: a namespace-scoped `DELETE_AND_RESTORE` deletes the whole namespace, ledger included, before it restores. An application-scoped restore deletes only what the application selects.
- **Selecting backed-up resources by an include label.** Rejected: a resource that misses the label silently drops out of every backup. The `NotIn [exclude]` selector includes unlabelled resources.
- **A daily-backup freshness gate on updates.** Rejected: the Pre-update Backup gives a newer restore point, and the existing alert already watches the daily plan.

## Consequences

- No Backup for GKE backup contains the ledger. Its only protection is its regional disk (ADR 0067) and the `memory-recovery` archives on the same disk; #434 tracks a copy outside that disk.
- The update entry records `pre_update_backup`. It records `writers_started` before any writer starts again after the backup: the new release's writers, or the old release's writers when a failure unseals. The runbook restores a Pre-update Backup only when `writers_started` is absent.
- The backup wait is at most 30 minutes, and it also stops early enough to leave 20 minutes of the 40-minute reconcile budget for the deploy.
- The deploy identity holds `roles/gkebackup.delegatedBackupAdmin` on the Pre-update Backup plan only. It can create, read, update and delete that plan's backups, but cannot change a plan or restore; the 2-day delete lock keeps it from deleting a new backup. Its `roles/container.admin` grants no Backup for GKE permission. People run the restore.
- Production deploys only the current protected `main`, so a restore goes with a release that reverts the failed one. That release's update takes its own Pre-update Backup of the restored state.
- Daily backups need the ProtectedApplication. The controller applies it in `kube.prepare` and before each Pre-update Backup. Until the first production deploy after bootstrap step 7, the daily plan has no application to back up.
