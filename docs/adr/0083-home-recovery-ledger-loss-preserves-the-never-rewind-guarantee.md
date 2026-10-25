# Home recovery ledger loss preserves the never-rewind guarantee

Decided on 2026-10-23 in the [#434](https://github.com/kent8192/aidash/issues/434)
grilling. Losing the Home recovery ledger disk must not rewind the Home epoch,
unit revision floors, body digests or permanent deletion fences. Recovery must
preserve conservative floors that became durable before a failed database
commit, too. A replacement disk may resume native memory only after the complete
required floors are proved; otherwise native memory stays unavailable.

The supported failure for this decision is loss or corruption of the ledger
disk while the current, unrewound PostgreSQL database remains available. Database
rewind continuity remains with #433. ADR 0082's exclusion of the ledger from
Backup for GKE and ADR 0067's acceptance of region loss remain in force.

Off-disk body archives follow the same expiry, current bank policy revision and
deletion rules as managed local archives. They receive no blanket retention lock
that prevents those rules from taking effect. Independently protected epoch and
floor information has a different purpose and lifetime from body archives; an
expired body archive cannot remove a permanent deletion fence.

GCS is the chosen independent durable store for the Home epoch and ordered,
non-body floor records. Record the required floors there before advancing the
local ledger and committing the database mutation. This adds GCS communication
to memory mutations in the protected profile. First-release lost-ledger recovery
is supervised: a person runs disk reconstruction and reconciliation, reviews the
result and resumes serving only when the Home-wide completion conditions hold.
Uncertain bodies remain withheld.

When GCS is unavailable, floor-advancing operations stop. Reads may continue only
against a local ledger whose independent publication is proved complete. An
unknown GCS write outcome or incomplete local publication closes native memory
for the affected Home until reconciliation proves its current floors.

Production creates an archive for every bank every six hours, copies it off the
ledger disk immediately and prunes off-disk archives every fifteen minutes.
Scheduled local archives are a bounded transfer queue: remove a file only after
verifying its remote contents, and report transfer failure or capacity exhaustion.
Expiry and policy invalidation still remove an unsent body archive.
The accepted ledger-only recovery target is two hours from fault occurrence,
with a lost-ledger drill before production activation and every quarter. This is
a target awaiting implementation and live evidence, not a measured result.

Keep the non-body safety history without expiry while the Home operates. The
first release does not automatically delete or compact that history, and normal
writers cannot overwrite or delete its existing records. Archive failures are
reported and retried rather than blocking normal operation or updates. Notify
when a bank's verified copy is over eight hours old, pruning fails for thirty
minutes, or its transfer queue exhausts capacity.

Recovery reconstructs the complete floors from GCS first, then checks current
PostgreSQL bodies against them. An eligible archive may supply a missing body
only when it matches those independently recovered floors and current authority.
A current database and complete safety history can therefore recover without
an eligible body archive. Never infer a missing floor from a database body.

## Considered Options

- **Copy body archives on a schedule and trust the latest copy after disk loss.**
  Rejected: an archive does not contain the Home's complete floors, including
  deletions and corrections after its creation.
- **Reconstruct floors from current PostgreSQL memory rows.** Rejected: those
  rows do not preserve every conservative floor advanced before a failed commit.
- **Allow an archive to rewind deletion or revision floors after total loss.**
  Rejected: disk loss does not relax the never-rewind guarantee.
- **Commit a dedicated floor ledger in a separate PostgreSQL transaction.**
  This could protect the accepted disk-only failure and retain floors after a
  failed business commit. GCS was chosen to keep the safety information outside
  database snapshots and avoid adding its rewind protection to #433. The trade-
  off is a new synchronous dependency for memory mutations.
- **Automatically replace the ledger disk and resume.** Deferred: the first
  release makes reconstruction and reconciliation reviewable by a person.

## Consequences

GCS protection of the epoch and floors is required in addition to scheduled
body archives. The publication and recovery protocols and acceptance cases are
recorded in [the recovery design](../operations/home-ledger-recovery.md).
These are accepted design decisions, not an implemented or accepted production
recovery capability. GCS safety history remains separate from the body-archive
bucket and its deletion identity. A lost ledger is rebuilt closed and cannot
reopen after reconciling only one bank.
