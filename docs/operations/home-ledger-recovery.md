# Home recovery ledger protection and recovery

Status: design decisions accepted on 2026-10-23 in
[#434](https://github.com/kent8192/aidash/issues/434). The implementation provides the protected profile, management commands,
scheduled Jobs and drill helper described below. Production apply, activation,
notification delivery and live lost-ledger acceptance remain pending.

## Accepted decisions

| Decision                      | Agreed behavior                                                                                                                                                                                                                                                      |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1: safety                    | Preserve the Home epoch, revision floors, canonical digests and permanent deletion fences after total ledger disk loss. Preserve conservative floors from failed SQL commits, too. Missing proof keeps native memory unavailable.                                    |
| Q2: failure boundary          | Support ledger disk loss or corruption with current, unrewound PostgreSQL and the independent GCS safety history intact. Database rewind continuity remains in #433. Keep ADR 0082's Backup for GKE exclusion and ADR 0067's accepted region-loss boundary.          |
| Q3: body expiry               | Apply existing managed-archive expiry and current bank policy invalidation/deletion rules to off-disk body copies. Do not impose a blanket retention lock that prevents those rules.                                                                                 |
| Q4: independent durability    | Synchronously record the epoch and ordered, non-body floor changes in GCS before advancing local floors and committing a database mutation.                                                                                                                          |
| Q5: supervision               | A person supervises disk reconstruction, all-bank reconciliation and resume. Uncertain bodies stay withheld. Automatic recovery from total ledger loss is deferred.                                                                                                  |
| Q6: GCS outages               | Stop floor advancement when independent durability cannot be established. Allow reads only from a local ledger proved fully published. An unknown write outcome or incomplete local publication closes native memory for the affected Home until reconciliation.     |
| Q7: archive schedule          | Back up every production bank every six hours, copy each archive off disk immediately, and prune remote body copies every fifteen minutes under current policies.                                                                                                    |
| Q8: local spool               | Bound the scheduled transfer queue. Remove a valid local archive only after verifying the remote contents. Expired or policy-invalidated files are still pruned even if unsent. Report transfer failures and exhaustion.                                             |
| Q9: recovery target           | Ledger-only recovery has a two-hour RTO from fault occurrence. Run a lost-ledger drill before production activation and every quarter. No live recovery result is claimed.                                                                                           |
| Q10: safety-history retention | Keep non-body safety history without expiry during Home operation. No automatic history deletion or compaction in the first release. Normal writers cannot overwrite or delete existing records.                                                                     |
| Q11: archive failures         | Notify and retry archive failures; do not block normal operation or routine updates. Notify on a verified-copy age over eight hours, thirty minutes of prune failure, or spool exhaustion.                                                                           |
| Q12: body recovery            | Rebuild floors from GCS, then prefer matching current PostgreSQL bodies. Use eligible archives only to supply matching missing bodies. An eligible archive is not required when current bodies and complete safety history suffice. Never derive floors from bodies. |

[ADR 0083](../adr/0083-home-recovery-ledger-loss-preserves-the-never-rewind-guarantee.md)
records the trade-offs. These decisions extend the
[managed recovery design](../memory-recovery.md) without permitting a database
rewind or a replacement epoch for an existing Home.

## Why periodic archives are insufficient

The current archive stores one bank's canonical units, Home, epoch, provider,
bank settings revision and validity interval. It does not contain the complete
Home ledger or changes after creation. Current restore requires an existing
epoch and a matching independently retained fence for every admitted unit.

The current transaction preflight fsyncs local floors before SQL commit. A
failed SQL commit can leave a revision or deletion floor ahead of the canonical
database row. Reconstructing floors from those rows would lose that protection.
The GCS history must therefore preserve attempted changes that became durable,
not only successful database commits. Source inspection used
`429410771bb30e0e4db5d840789066dcbaeadc62`.

The two-hour target applies to ledger disk loss with current PostgreSQL intact.
Zero rewind of safety floors is independent of the six-hour body-archive
schedule. The schedule is not a zero-loss body restore guarantee, and it does
not activate #433's daily-database-backup recovery target.

## Engineering choices for the protected profile

The following choices implement the accepted constraints.

- Keep the existing single-attach, shared local filesystem and its cross-process
  Home lock. This work does not introduce replicated server/worker writers or a
  distributed writer lease.
- Use separate Terraform-managed safety-history and body-archive buckets in the
  existing `us-central1` production region, with uniform access, public access
  prevention and destruction guards. Region loss remains accepted.
- Give the safety writer create/read/list access without delete or overwrite.
  Give body maintenance its own identity and body-only deletion access. Keep
  bucket administration and supervised recovery with the Deployment Administrator
  boundary. Do not reuse a broad server credential for scheduled Jobs.
- Use immutable, create-only objects with generation preconditions. Bound each
  object and local read by the existing 64 MiB file guard. A large initial Home
  seed uses multiple chunks and a final manifest; it must not impose a 64 MiB
  aggregate Home limit.
- Keep minimal safety fields: format, Home, epoch, ordered sequence, predecessor
  digest, operation identity, publication generation/mode, and each affected
  unit's identity, bank, revision, deletion state and canonical digest. Do not
  copy bodies, credentials, prompts or authority snapshots into safety records.
  A digest is not a body backup.
- Use a separate local scheduled spool so retained manual archives cannot fill
  the schedule's queue. Start with adjustable guards of 128 pending files and
  512 MiB; these are engineering defaults, not approved service capacity targets.
  Keep the existing per-bank archive file guard. Oversized or otherwise invalid
  banks produce explicit failures rather than a successful all-bank receipt.
- Use separate Kubernetes CronJobs for archive generation and pruning, fixed
  image digests and non-overlapping runs. The backup Job mounts the Home claim
  on its owning Cluster Node. The prune Job does not need the ledger disk and
  must remain runnable when that disk is lost. Grant each Job only its required
  database scope, Workload Identity and network access.
- Require the protected profile for production; allow explicit nonproduction
  opt-in to exercise it. Ordinary local development can keep the existing
  profile. Once a Home requires independent protection, a missing setting or
  unavailable GCS must not silently select local-only writes.

## Establishing independent protection

For an existing Home, close native-memory admission, drain or stop every
floor-advancing writer, including workers, maintenance and recovery commands,
then snapshot the complete local epoch and every unit shard. Seed GCS from that
ledger, including conservative floors whose SQL transaction failed and identities
no longer present in current bank rows. Never seed from canonical bodies alone.

Write and verify all seed chunks before creating the final immutable manifest.
That manifest is the complete seed's publication point. Persist the protection-required and activation markers before publishing the
seed so an interrupted activation stays closed. Verify the complete seed before
persisting the applied history position and restoring the prior serving gate.
No old binary or local-only writer may resume between seeding and activation.
Failed or partial activation remains closed and resumable with the same epoch.
Do not hold the filesystem lock while waiting for SQL locks in the reverse of
the existing transaction lock order.

For a genuinely empty new Home, independently publish its epoch before it can
advance local floors or admit native memory writes. An existing GCS anchor,
partial local state, or a nonempty Home must never cause initialization to mint
a replacement epoch. A protected server/worker startup verifies local protection
and publication state before serving memory. Failure leaves memory unavailable
while ordinary management remains usable.

Provisioning the buckets and identities is not activation evidence. Production
activation additionally requires the archive/prune checks and lost-ledger drill
below. Existing-Home activation is quiesced; online migration is not required.

## Publishing a floor change

Every floor-advancing path uses the same publication protocol: ordinary commit
preflight, `into_executor`, restore's direct `advance_restore`, and cleanup that
changes the digest of a hidden or stale body at the same revision. A maximum
revision number alone is insufficient. Preserve transition ordering and deletion
permanence through the existing domain validation rules.

1. Under the existing writer locks, validate the entire proposed batch against
   current epoch and floors. Establish the expected history predecessor. Never
   lower a floor or authorize an unknown unit from an archive.
2. Under the Home publication lock, fsync a local pending record identifying
   the operation, predecessor and intended batch. Other processes and restarted
   processes must observe it. A pending record cannot masquerade as a clean
   publication position.
3. Write immutable GCS payload chunks if needed, then a create-only commit record
   that binds their exact generations/digests and history predecessor. Only a
   complete commit record advances the independent floor history. The protocol
   must detect gaps, conflicting records and epoch mismatches. The filesystem
   writer lock and create preconditions prevent accepting two successor batches
   at the same sequence; a conflict requires refresh and validation.
   Allow at most one unresolved commit operation per Home. A recovery barrier
   closes ordinary publication; an old writer cannot skip that barrier and
   continue at a later sequence or publication generation.
4. Confirm that exact committed operation. Fsync every affected local shard and
   the applied history position, then durably clear the pending record. Keep
   native memory closed if any local publication step fails.
5. Commit the SQL transaction. If that commit fails, keep the independently
   published and locally applied conservative floors. Do not compensate by
   deleting a record, decrementing a floor or changing the epoch.

Reads may continue when the local state is clean and independently published.
If a request is definitively rejected before any possible publication, reject
the mutation without changing those proven floors. A timeout or lost response
does not prove rejection: keep the pending state and close that Home's native
memory until the exact operation is resolved. A single `404` after a timeout
does not prove an earlier request cannot still complete. Retry the same operation
identity and sequence with the same content until its committed outcome is
positively established; never bypass it with a later sequence.

Incomplete publication can be reconciled automatically when its result is proved.
Supervision is required for total disk reconstruction. Always reconstruct a lost
disk with a closed gate; replaying an earlier open gate does not authorize resume.
Retain current authority, source, retention and dependency rechecks on reads.

## Six-hour all-bank archive workflow

Enumerate every configured bank for the Home through `memory_bank_settings`
joined to `memory_banks`, including shared banks whose participant is absent.
Use stable pagination. Record the inventory cut and evaluate new banks against
their first due schedule rather than claim that a run covered future banks.

For each bank, acquire the existing workspace lock and re-read current bank
settings and the pinned policy. Use the existing canonical-unit, fence, origin
and authority checks to produce the archive. Re-check eligibility on retry.
Continue processing other banks after a bank fails, and record each failure;
only a fully accounted inventory may produce an all-bank success receipt.

Write to the bounded spool, immediately upload with create-only semantics, then
verify the object at its exact generation against the local digest and validity
metadata. Only that verified copy counts toward freshness. Preserve the original
archive creation time; a later upload or retry does not renew retention or make
an old archive fresh. Re-check policy invalidation before accepting the copy.

Remove a still-valid local spool file only after verification. Independently
delete expired or policy-invalidated local files even if they were never sent.
Do not silently evict valid unsent files to make room. Exhaustion fails that
bank's attempt and triggers the accepted notification. Archive retries must not
starve later banks or monopolize the publication lock.

## Fifteen-minute body pruning

Run remote pruning independently of archive generation, local ledger health and
the native-memory serving gate. Read current policies from unrewound PostgreSQL.
The existing `prune_expired` path cannot be reused unchanged: a missing ledger
currently stops it, and its error short-circuits later background maintenance.

Retire copies when their recorded expiry is reached, current `backup_days`
shortens their lifetime, their exact settings revision/provider is superseded,
or their bank is no longer eligible. Use generation-aware deletion and isolate
per-object failures so a corrupt archive cannot prevent cleanup of every bank.
An archive never becomes restorable merely because cleanup was delayed.

Create the body bucket with soft delete explicitly disabled, versioning disabled,
no holds and no bucket/object minimum-retention requirements. Verify those
conditions during activation.
Otherwise a successful DELETE can leave recoverable versions that contradict
the accepted body-copy expiry. The safety bucket has separate policy and IAM;
body pruning must not delete any permanent floor or safety-history object.

A GCS outage can prevent physical deletion. Record the actual outstanding work
and notify at the accepted thirty-minute threshold; do not claim expiration
removed recoverable bytes until deletion succeeds. Current policy still rejects
restoring or disclosing an expired copy immediately.

## Supervised recovery after ledger disk loss

1. Record the fault time, affected Home, running revision/images and last known
   publication position. Stop and fence old writers and isolate the damaged
   volume. Keep current PostgreSQL, Registry, authority and other durable services
   in place; do not restore a database snapshot for this procedure.
2. Verify the configured Home anchor, epoch and complete GCS seed/history. After
   stopping/fencing old writers, claim a recovery barrier in the next sequence
   slot using the same create-only condition as an ordinary commit. If an old
   in-flight commit wins that slot, verify and include it, then claim the next
   slot. The barrier is the stable recovery cut: delayed old requests cannot
   overwrite it, and ordinary writers must stop at it rather than skip ahead.
   Do not infer this cut solely from elapsed time or an empty listing. Validate
   every referenced generation, digest, predecessor and committed batch through
   the barrier. Ignore incomplete payload chunks without a commit record, but
   never ignore a missing or conflicting committed record. Missing proof leaves
   the Home closed.
3. Rebuild the replacement directory with the same epoch, a closed gate,
   protection-required state and every resulting permanent fence. Retain floors
   for absent SQL rows and retired banks, including failed additions/deletions.
   Make the reconstruction resumable after a crash. Never call ordinary `init`
   on an existing or restored nonempty Home.
4. Reconcile every current bank and all independently recorded unit identities.
   Prefer current SQL bodies whose identity, bank, revision and canonical digest
   match the recovered floor. Keep authority, Registry, bank/participant identity,
   writer origins, Messages, Artifacts, Run journals, budgets and request/deletion
   receipts current. Revalidate origin lineage, source evidence, current writer
   authority and retention. Preserve conservative uncertain charges.
5. If a current body is unavailable, consider only an unexpired managed archive
   for the same Home, epoch, bank and current policy. Adopt a body only when it
   matches the recovered floor and the current source/origin/authority checks.
   Withhold uncertain or missing bodies; do not invent a replacement body, origin,
   permission or floor. Absence of any eligible archive is not itself a failure
   when current SQL bodies suffice.
6. Rebuild or verify semantic projections from the accepted current bodies.
   Prove deleted, superseded, withheld and unauthorized bodies stay absent from
   recall, dependent reads and execution. One bank's success cannot reopen the
   entire Home. Require a complete inventory, no unresolved publication, and an
   explicit report of every restored/withheld identity before resume.
7. The supervising person reviews the report and runs the resume operation,
   which revalidates the same Home-wide completion evidence before opening the
   gate. During reconciliation only supervised recovery work may append after
   the barrier, through the same durable floor protocol. Final resume records
   the completed reconciliation and new publication generation before opening
   the local gate; old-generation writers cannot resume ordinary publication.
   Verify authorized reads/writes and required-memory execution behavior, then
   record elapsed RTO and actual recovered/withheld counts. Crossing two hours
   fails the recovery target; it never permits skipping proof.

## Notifications and evidence

Use the existing production operations notification channel and runbook owner.
Notify immediately for an unresolved publication that closes a Home, and for
spool exhaustion. Notify for a bank whose latest eligible verified copy is over
eight hours old, or when pruning cannot complete for thirty minutes. A newly
eligible bank starts its initial freshness clock at eligibility and must be
copied by its first due six-hour run; a policy change cannot keep counting an
invalid older copy. Keep a durable first-failure/last-success observation so a
restarted Job does not reset the clock. Missing executions must also be detected.

Keep metric labels bounded to deployment/profile rather than Tenant, workspace
or unit. Put bank-level failures and object generations in restricted diagnostic
reports, with no body or secret values. Archive failures do not gate routine
updates; synchronous protection and production activation requirements still do.

Before production activation and quarterly thereafter, run the isolated drill
with current unrewound test PostgreSQL and all local epoch, ledger, unit shards,
publication markers and archives lost. Cover an archive older than a subsequent
correction/deletion, an independently durable floor whose SQL commit failed,
missing eligible archives, and multiple banks. Verify intermediate restart,
lost GCS response, incomplete local publication, corrupt/missing history records,
an old GCS commit delayed until after recovery barrier creation, policy
invalidation and full spool behavior. Also prove the clean-read GCS-outage case
and rejection of floor advancement during that outage.

Record revision/image digests, topology and tested volume, fault/detection/resume
times, history cut and object generations, expected unit identities/revisions,
restored/withheld counts, authorization and semantic checks, actual RTO, manual
actions and notification delivery. GCS upload success, Terraform validation,
hosted CI and supervised recovery acceptance are separate evidence claims.
The target remains unverified until that complete drill passes for the recorded
production profile; this document creates no throughput or scale guarantee.

Coordinate the case with #435's disposable drill without changing its supported
Pre-update Backup boundary. PR #473 was still open at the 2026-10-23 source
inspection; its existing-ledger drill does not by itself prove this case. Neither
this procedure nor its RTO makes daily database rewind a supported recovery.

## Implementation boundaries

Keep pure floor/history validation in domain, ports/use cases in application and
GCS authentication/transport in external adapters. Wire the protected profile,
persistence, commands and Jobs through server and deployment composition. Domain,
application and harness must not acquire cloud SDK or HTTP client dependencies.
Use existing migration and ORM rules for any durable observations or new state.

Local regression tests cover unknown outcomes, delayed old writers, failed SQL
commits, seed retries, stale-body rejection, all-bank coverage, policy expiry,
Job isolation and lost-ledger reconciliation. Live drill checks remain required. Keep
the existing local profile's failure behavior unless the protected profile is
explicitly active. Command names, retry intervals within the accepted deadlines,
object encoding and internal module boundaries can be settled by the implementer.

## Commands and production composition

Set the same Home identity, database and absolute directory as the server. The
protected profile adds these settings:

| Setting                                  | Purpose                                                                             |
| ---------------------------------------- | ----------------------------------------------------------------------------------- |
| `AIDASH_MEMORY_RECOVERY_REQUIRED=true`   | Require independently published floors even before the durable local marker exists. |
| `AIDASH_MEMORY_RECOVERY_HISTORY_BUCKET`  | Create-only safety objects under the hashed Home prefix.                            |
| `AIDASH_MEMORY_RECOVERY_ARCHIVE_BUCKET`  | Expiring body copies; separate from history.                                        |
| `AIDASH_MEMORY_RECOVERY_SPOOL_MAX_FILES` | Scheduled spool file limit, default 128.                                            |
| `AIDASH_MEMORY_RECOVERY_SPOOL_MAX_BYTES` | Scheduled spool byte limit, default 536870912.                                      |

The ordinary local profile remains available outside production. Removing
settings after protection has been established cannot override the on-disk
`protection-required` marker. Protected startup verifies the complete history;
an incomplete pending append is retried with its original identity and sequence.
A failed verification closes memory while management remains available.

Bootstrap Terraform provisions fixed custom storage roles and the deployment
identity's limited bucket creation/configuration permissions. Environment
Terraform provisions the separate buckets and Workload Identities. Production
always enables the protected values. Nonproduction requires both
`memory_recovery_nonproduction_enabled=true` in bootstrap and
an Environment opt-in in the trusted controller's `AIDASH_GCP_CONFIG`, for
example `"recovery_protected": {"test": true, "pr-42": true}`. Keys are exact
Environment IDs (`develop`, `test`, or `pr-N`). The next authorized deployment
sets `recovery_protected=true` in that Environment's desired Terraform
configuration and retains it across updates and stop/resume. Removing the
opt-in or setting it to `false` does not downgrade an already protected Home.
Apply bootstrap before the Environment resources. This provisioned configuration
is not evidence of activation or a successful restore.

The Helm `memoryRecovery.protection` values select the buckets, dedicated
backup/prune service accounts and `app-memory-recovery` Secret. The controller
creates a separate PostgreSQL maintenance login with read access and column
privileges needed for PostgreSQL `FOR SHARE` locks. Owner-managed row triggers
reject every actual mutation by that login. It cannot own/alter schema, inherit
the server/owner role or disable those guards. Jobs receive its database URL and
an isolated configuration-validation token that no server admits. The shared
settings loader derives its otherwise unused framework key from that token.
Jobs do not receive the server's runtime Secret or migration credentials. The
backup identity reads history and creates/verifies body objects; only the prune
identity deletes body objects, and it has no safety-history grant.
The chart installs both schedules suspended. The controller resumes them after
successful deployment or reload initializes the schema and maintenance login,
and suspends them before stopping the Environment's dependencies. Their network
access to those dependencies permits PostgreSQL alone.

Both Jobs mount the same public GCIP settings as the serving processes and build
the same Dashboard AccountPolicy. Saved Subject and Run authority is rechecked
under policy/credential locks without writing authorization audit records or
disabling live Identities. Refused authority still rejects that bank's backup.

The migration Job needs the metadata-server network label when protection is
required so new-Home initialization can establish its GCS anchor. Prune has that
network label and its own identity but no ledger claim or co-location affinity.
Its policy lookup does not consult the local ledger or serving gate. Corrupt
objects and per-object failures do not short-circuit cleanup of other objects.

After sealing admission and stopping every floor-advancing server, worker and
maintenance command, activate an existing Home with the candidate binary and
its configured history bucket:

```sh
aidash memory-recovery protect --directory "$AIDASH_MEMORY_RECOVERY_DIR" --writers-fenced
aidash memory-recovery status --directory "$AIDASH_MEMORY_RECOVERY_DIR" --json
```

`protect` seeds every local shard, including conservative floors absent from
PostgreSQL. Interrupted seeding can be retried. A previously closed restore gate
stays closed; this command cannot finish an unrelated recovery. For an empty new
Home, `init-if-missing` publishes the anchor before creating the local epoch.
Existing unprotected/corrupt state fails that bootstrap until supervised repair.

The scheduled commands also work as explicit maintenance invocations:

```sh
aidash memory-recovery backup-all --directory "$AIDASH_MEMORY_RECOVERY_DIR"
aidash memory-recovery prune-remote --directory "$AIDASH_MEMORY_RECOVERY_DIR"
```

They print structured receipts and return failure for an incomplete inventory or
prune. A failed bank does not stop the next bank. The spool retains a valid file
when an upload acknowledgement is lost; retry verifies its exact remote generation
before removing it. Invalid/expired unsent files are removed. Oversized banks and
spool exhaustion produce an explicit failed result. Manual `backup` archives
remain in `archives/` with their separate existing 128-file guard.
The locked spool scan reclaims recognized abandoned atomic-write temporaries;
all remaining entries count toward its bounds, including unknown regular files.

The fifteen-minute prune checks current copies and emits stale-bank observations
after eight hours from the last verified archive's original creation time, or
from the bank's durable `recovery_eligible_at` when no eligible copy exists. A
policy replacement starts a new eligibility interval. Job restarts cannot reset
that timestamp. Structured failure/stale-copy receipts notify the existing
production mailbox. Cloud Monitoring retains prune-success observations across
Pods and alerts when none occurred in thirty minutes, including Jobs that never
start. An unresolved floor publication or failed protected startup emits a
sanitized `memory_recovery_closed` event and immediately notifies the same
destination. The safety alert selects serving/migration containers, so the prune
Job's deliberately absent ledger does not trigger it. Exercise the first
successful observation and all three notification paths during
activation; configuration and mock-provider tests do not prove delivery.

For a lost ledger, keep current unrewound PostgreSQL, fence every old writer and
mount a new empty directory with the same UID/GID and Home identity. Do not run
`init`. Use the safety writer's supervised identity for reconstruction/resume:

```sh
aidash memory-recovery reconstruct --directory "$AIDASH_MEMORY_RECOVERY_DIR" --writers-fenced
aidash memory-recovery reconcile-all --directory "$AIDASH_MEMORY_RECOVERY_DIR" --writers-fenced \
  --report "$AIDASH_MEMORY_RECOVERY_DIR/reconciliation.cbor"
aidash memory-recovery resume --directory "$AIDASH_MEMORY_RECOVERY_DIR" --writers-fenced \
  --report "$AIDASH_MEMORY_RECOVERY_DIR/reconciliation.cbor"
```

`reconcile-all` keeps the gate closed and reports the complete bank inventory,
applied head and restored/withheld counts. Absent and retired identities remain
accounted for by their retained floors. Current matching bodies take precedence;
the archive bucket is not contacted when those bodies suffice. If optional
archive lookup is unavailable, the report says `archive_unavailable`; uncertain
bodies remain withheld. Resume rechecks all-bank authority, sources, retention,
semantic projections, inventory and the same history head. Changed results need
a fresh report. An identical resume can finish after a lost acknowledgement
while the Home is still reconstructed. A completed report cannot reopen a later
ordinary restore; complete that restore's reconciliation before resuming it.

## Executable drill evidence

[The drill helper](../../scripts/home_ledger_drill.py) sequences the same commands
and writes a private JSON receipt outside the tested disk. It requires Python 3
and the candidate `aidash` binary in a supervised maintenance environment with
its PostgreSQL, GCS identity and volume mounts. The normal application image does
not include Python. Run the helper in a maintenance image built from the same
application revision, or run the commands above and retain equivalent evidence.

Record the protected Home before the fault:

```sh
python3 scripts/home_ledger_drill.py record --binary aidash \
  --directory "$AIDASH_MEMORY_RECOVERY_DIR" --evidence /evidence/lost-ledger.json
```

Stop all writers, record the actual fault time and replace the disk outside this
program. Then run:

```sh
python3 scripts/home_ledger_drill.py recover --binary aidash \
  --directory "$AIDASH_MEMORY_RECOVERY_DIR" --evidence /evidence/lost-ledger.json \
  --fault-at 2026-10-23T00:00:00Z --writers-fenced
```

Replace the example timestamp with the observed fault time. The helper refuses
to delete an existing ledger, reconstructs behind a barrier, saves the all-bank
report, resumes, verifies the same epoch and open serving state, and measures
RTO from the fault. Interrupted recovery keeps the original clock and report;
rerun with the same evidence and omit `--fault-at`. If Resume already completed,
the helper verifies the matching Home, epoch and immediate successor head before
finishing the receipt without resubmitting the completed report. Later writes or
a new restore prevent that completion shortcut. A missed two-hour target is
recorded as a failed drill even if recovery eventually succeeds.

The receipt supplements the required image/revision, canary, object-generation,
volume/topology and notification evidence above. It cannot certify those external
facts by itself. Run the pre-activation and quarterly drills under Deployment
Administrator supervision and attach their receipts to the release evidence.

## Primary references

- [Cloud Storage consistency](https://docs.cloud.google.com/storage/docs/consistency):
  object reads and listings are strongly consistent; this is not a multi-object
  transaction or proof of application-level history completeness.
- [Request preconditions](https://docs.cloud.google.com/storage/docs/request-preconditions):
  use create-only and exact-generation operations for retries and verification.
- [Cloud Storage IAM roles](https://docs.cloud.google.com/storage/docs/access-control/iam-roles):
  object creation can be granted separately from read/list and deletion.
- [Bucket Lock](https://docs.cloud.google.com/storage/docs/bucket-lock): locked
  minimum retention prevents deleting younger objects and cannot be reduced.
- [Soft delete](https://docs.cloud.google.com/storage/docs/soft-delete): supported
  new buckets default to retaining deleted objects; disabling it later does not
  remove copies already retained under the earlier policy.

- [PostgreSQL SELECT privileges](https://www.postgresql.org/docs/current/sql-select.html):
  row-locking clauses require an UPDATE privilege in addition to SELECT.
- [Cloud Monitoring PromQL mapping](https://docs.cloud.google.com/monitoring/promql):
  the prune heartbeat aggregates persisted log-derived metrics across disposable Jobs.
