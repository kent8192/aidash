# New-format memory recovery

This guide covers the managed memory-only recovery profile for Issue #125.

Each logical Home has a dedicated persistent `AIDASH_MEMORY_RECOVERY_DIR`.
HTTP and worker processes for the same Home share this directory. PostgreSQL
backups must not rewind this external directory. Its epoch anchor and revision,
body-digest and deletion ledger are independent from the database. The initial
profile requires local filesystem locking, atomic rename and file/directory
fsync; a different shared filesystem needs its own acceptance evidence.

`epoch.cbor` anchors the Home identity and `ledger.cbor` stores its serving
gate. Permanent unit fences live in `units/<unit-uuid>.cbor`, each bound to
that epoch and protected by a checksum. A write validates the entire batch,
then atomically replaces and fsyncs each affected fence before the database
commit. Partial filesystem failures retain conservative floors and withhold
uncertain bodies. Normal reads inspect only the selected unit's fence;
recovery and status combine the shards. The 64 MiB per-file guard therefore
does not impose an aggregate Home unit capacity. Retain the whole directory,
including every unit shard, independently of database snapshots.

Initialize once after native schema creation, before creating native memory
participants or banks. Initialization requires empty new-format memory and
does not import existing units or JSON memory:

```sh
cargo run -p aidash-server --bin aidash -- memory-recovery init \
  --directory /absolute/persistent/home-memory-recovery
```

Compose, `cargo make k8s-up` and the GCP host installer run `memory-recovery init-if-missing` after
migrations and before starting the app. This explicit bootstrap initializes only
an empty new-format Home with no ledger or epoch anchor. It validates existing
state and preserves a closed serving gate. Partial, corrupt or mismatched state
fails deployment bootstrap and requires recovery; it never replaces an epoch.

Set `AIDASH_MEMORY_RECOVERY_DIR` to that same absolute path when starting the
Home HTTP server and workers. Missing, corrupt, mismatched or closed external
state makes native memory unavailable; primary execution and management remain
usable. A missing ledger never causes serving to initialize a replacement epoch.
Do not repair a missing volume by running `init` again.

For Helm, provision one Home-owned claim and initialize its directory with the
application UID/GID 10001. Set `memoryRecovery.existingClaim` to this claim and
`memoryRecovery.directory` to the absolute mounted directory. The chart mounts
the same claim in the server and worker. If the initialized Home is a child of
the claim root, set `memoryRecovery.subPath` to that relative directory. The
local Kubernetes initializer creates `home/`, and `cargo make k8s-up` mounts
that child at `/var/lib/aidash/memory-recovery/home` for both roles. The chart
never initializes or replaces an epoch. Run the one-time command in an operator
Job with the same image, database
credentials, Home identity and volume. A newly mounted volume root may be owned
by root: create/chown the Home directory before running `init`, rather than
depending on `fsGroup` to grant permission to chmod the volume root. Separate
Home identities must use separate claims/directories.

Create a managed archive for the exact bank. Omit `--participant` for the shared
Workspace bank. Archives contain typed CBOR canonical units and exact source
references, scoped to the current bank/policy and external epoch:

```sh
cargo run -p aidash-server --bin aidash -- memory-recovery backup \
  --directory /absolute/persistent/home-memory-recovery \
  --tenant tenant-id --workspace workspace-uuid --participant participant-uuid
```

The command prints the archive path. The pinned policy's `backup_days` governs
expiry. Backup/prune operations also retire archives whose exact bank policy
revision has been superseded, and enforce the current policy's retention from
the archive creation time. Expired managed files are physically removed by these operations
and ordinary background maintenance. Archives have a 64 MiB file cap and the
managed directory has a 128-archive admission cap. These are explicit storage
guards, not approved Issue #73 service or performance targets.

Recovery restores memory into an existing live Home database. Keep current
authority, Registry, participant/bank identity, writer-origin records, primary
Messages/Artifacts/Run journals, budget reservations and request/deletion
receipts. This profile does not rewind those tables. Legacy JSON files and
legacy bank backups are unsupported inputs.

```sh
cargo run -p aidash-server --bin aidash -- memory-recovery prepare-restore \
  --directory /absolute/persistent/home-memory-recovery
cargo run -p aidash-server --bin aidash -- memory-recovery restore \
  --directory /absolute/persistent/home-memory-recovery \
  --archive /absolute/persistent/home-memory-recovery/archives/archive-uuid.cbor
```

`restore` also closes the persistent gate before taking database locks. A crash
or failed validation keeps memory unavailable across process restarts. Repeating
`restore` with the same valid archive/epoch resumes reconciliation; no command
opens the gate without reconciliation. An old body below the external revision
floor is withheld rather than guessed. Current source/authority, retention and
origin allowances are rechecked before index work and final disclosure. Rebuild
operations retain conservative charges for uncertain earlier attempts.

Indexing holds a shared workspace lock before reading source rows and through
the independent durable embedding reservation. Concurrent indexers can share
that lock; retention maintenance skips its workspace writer while those readers
are active, avoiding a cycle between source settings and reservation foreign keys.

Inspect the gate and remove expired archives with `memory-recovery status` and
`memory-recovery prune`, using the same `--directory`. Status reports the epoch,
serving gate and number of fenced unit identities. Restore reports restored and
withheld unit counts. These describe memory recovery. They do not prove cleanup of
previously disclosed information, hosted backups outside this managed directory,
or a PostgreSQL replica/failover profile. Full database rewind requires a separate
authority/control/evidence continuity design and acceptance result.

The disposable cluster runner creates an application-owned child directory under
each Home volume. Run `scripts/test-cluster.sh kubernetes native-memory`. This
command uses an explicit temporary kubeconfig and preserve the current context.
Their synthetic provider exercises real native Registry/HTTP/worker paths;
successful runs establish only the cases recorded in their evidence directory.
