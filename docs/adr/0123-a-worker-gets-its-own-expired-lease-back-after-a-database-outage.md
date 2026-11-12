# A worker gets its own expired lease back after a database outage

Decided on 2026-11-09 in the #786 grilling. A Lease Recovery is another worker taking a Run over. A worker that kept running through a database outage longer than its 30 s lease renews its own expired lease, as long as the lease still names it, and its Run is charged nothing.

Three changes make that hold:

- **The renewal fence.** The lease renewal requires only that `lease_owner` is still the worker. Every other worker write keeps requiring an unexpired lease, including the checks that a lease has time left before an outside call. A claim replaces the owner under the row lock, so a renewal never revives a lease another worker took.
- **A step that failed on the outage or hit an expired lease fence renews first.** Such a step stops renewing when it returns, so before it records the failure the worker renews its lease, waiting for the database. If renewal succeeds after a lease-lost conflict, the worker records a retry without charging a recovery. If another worker took the Run meanwhile, the failure is theirs and the worker drops it.
- **A hold on recoveries.** Every process meets an outage at once, and every lease held across it expires. For one lease period after a process last failed to reach the database, its recovery claims leave Runs whose expired lease still names an owner. New Runs, Runs with no owner and cancelled Runs are claimed at once. Lock waits, deadlocks and statement timeouts do not start the hold, because the database is up.

A worker cut off from the database while the others reach it still has its Run taken over and charged, because another worker really runs the step again.

## Considered Options

- **Fix only the Crash Loop Failure message.** Rejected: three outages of over 30 s during one step are rare, but each outage also turns every in-flight Run into an Isolated Step, so right after an outage each process runs its Runs one at a time.
- **The returning worker records that it lost its lease to an outage, and that recovery is not charged.** Rejected: the record can arrive after another worker has claimed the Run and charged it, so the count would have to be taken back, and it rests on the worker's own word.
- **Drop the expiry from every worker fence.** Rejected: some fences check that a lease has time left before an outside call, and changing them all reaches far beyond this failure.
- **Claim only leases expired for a while, always.** Rejected: that delays the recovery of every Run whose worker really died, not only those right after an outage.

## Consequences

- A process that dies right after an outage has its Runs recovered up to one lease period later.
- A worker whose step was cut off but kept running across an outage may write again once it renews. This is the same worker; nobody else ran the step in between.
- `aidash_worker_lease_reclaims_total` counts reclaimed leases, and a warning names each Run with how long its lease had expired.
