# New-format memory recovery

This guide covers the managed memory-only recovery profile for Issue #125. It
applies to memory units in the source ingestion and knowledge contracts (b) and
(c); it does not cover Run-context continuity (see [architecture](architecture.md#memory-contracts)).

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

## Change matrix

Issue #127 defines what each kind of memory change does to search, to Runs that
already read the changed memory and to content derived from it. The default is
fail-closed: no change narrows today's revision, authority or required-Home
checks unless a versioned contract change is approved. A caller cannot classify
its own change; content, change and mutation shapes reject unknown fields such
as a "metadata-only" or "index-only" label
(`a_caller_label_cannot_exempt_a_memory_change`).

Each cell is either **E** (existing behavior on `develop/0.1.0`, followed by the
test that proves it) or **P** (proposed; a versioned contract change that this
document does not implement). Unprefixed test names live in
`server/src/apps/knowledge/tests/native_memory.rs` or `change_matrix.rs` next
to it, and a file-stem prefix such as `decay:` names another file in
`server/src/apps/knowledge/tests/native_memory/`. `remote:` tests live in
`server/src/apps/identity/tests/scoped_remote_execution*.rs`, `harness:` tests
in `crates/aidash-harness/src/agent/tests.rs`, and `semantic:` tests in
`server/src/apps/knowledge/tests/semantic.rs`.

### Rows

1. **Content or metadata update**: `Change::Correct` of any field, including
   metadata-only edits and byte-identical content.
2. **Correction or withdrawal of a claim**: `Change::Correct` with
   `verification: contradicted`, or a withdrawal by deletion (row 4).
3. **Permission or credential revocation**: policy, catalog, Subject Credential
   or Binding revocation (ADR 0013).
4. **Deletion**: `Change::Delete`, semantic entry deletion, or retention expiry.
5. **Index maintenance or embedding-model change**: `Reindex`,
   `ConfigureIndex`, a new search generation.
6. **Usage-based archival and reactivation** (#140): the dormancy sweep, `Pin`,
   `Unpin`, `Reactivate`.
7. **Batch undo of autonomous admissions** (#141): not implemented.

### Columns per row

| Row | Authoritative initiator                                                                                                                                                                                                | Search eligibility                                                                                                                                                     | Source identity and revision                                                                                                                                                 | Read dependencies                                                                                                                                                                                                                                                                                                                                                                 |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | E: a Subject with `memory.write`, or the Run's own `memory_mutate` under its authority; compare-and-set on the observed revision, one winner (`concurrent_writers_on_one_unit_have_a_single_winner_and_fence_readers`) | E: only the current canonical revision is delivered; delivery rechecks it                                                                                              | E: same Unit identity, revision + 1 for every correction, including metadata-only and identical content (`every_unit_change_fences_a_dependent_run_regardless_of_its_shape`) | E: every Run read of the old revision is invalid, on every journal page (same test; `review_regressions: run_journal_above_1024_checks_revisions_on_later_pages`)                                                                                                                                                                                                                 |
| 2   | E: as row 1                                                                                                                                                                                                            | E: a contradicted Unit is neither recalled nor listed (`Unit::visible`; `…_regardless_of_its_shape::withdrawal`)                                                       | E: as row 1 (`…_regardless_of_its_shape::withdrawal`)                                                                                                                        | E: as row 1                                                                                                                                                                                                                                                                                                                                                                       |
| 3   | E: Operator or Tenant Administrator through policy, catalog or credential changes                                                                                                                                      | E: rejected before search; indexing jobs keep the revocation (`semantic: semantic_access_is_checked_before_search_and_jobs_retain_revocation`)                         | E: unchanged                                                                                                                                                                 | E: dependent journals are hidden (`semantic: semantic_context_is_provenanced_and_revocation_hides_run_journals`)                                                                                                                                                                                                                                                                  |
| 4   | E: a Subject with `memory.write`, or retention expiry (`memory_ttl_withholds_units_and_dependents_before_cleanup_and_after_restore`)                                                                                   | E: excluded immediately by the SQL tombstone, independent of vector cleanup (same test)                                                                                | E: the tombstone keeps the identity; it is never reused (`decay: retention_controls_replay_without_renewing_reactivation_and_reject_reused_ids`)                             | E: invalid (`…_regardless_of_its_shape::deletion`)                                                                                                                                                                                                                                                                                                                                |
| 5   | E: Operator (`ConfigureIndex`) or a memory writer (`Reindex`) with the expected index revision                                                                                                                         | E: the new generation replaces the old one                                                                                                                             | E: canonical revision unchanged                                                                                                                                              | E: remain valid locally (`home_run_reads_survive_reindex_but_the_writer_is_invalidated_by_its_own_correction`) and remotely for a same-specification rebuild (`remote: ordinary_native_remote_context_uses_home_units_and_deletion_stops_inference`). P: an embedding-model change under a pinned remote binding is expected to pause with `configuration`; no test proves it yet |
| 6   | E: the dormancy sweep under the pinned decay policy; `Reactivate` by a person only (`memory_decay: person_only_actions_reject_agent_tokens_even_with_explicit_grants_and_dormant_reads_default_deny`)                  | E: excluded from ordinary recall, returned by `recall_including_dormant` (`decay: dormant_is_recall_only_and_pin_correction_and_manual_reactivation_preserve_content`) | E: revision unchanged (`decay: dormancy_and_reactivation_keep_existing_run_dependencies_valid`)                                                                              | E: remain valid (same test)                                                                                                                                                                                                                                                                                                                                                       |
| 7   | P: a person with `memory.write` on the Bank, never the admitting Agent                                                                                                                                                 | P: excluded immediately, as row 4                                                                                                                                      | P: each admitted revision is deleted by compare-and-set; a Unit corrected after admission is reported, not undone                                                            | P: invalid, as row 4                                                                                                                                                                                                                                                                                                                                                              |

| Row | In-flight inference and result adoption                                                                                                                                                                                                                                                                      | Run continuity                                                                                                                                                                                                   | Derived candidates, memories and artifacts                                                                                                                                                                                                                                                                                                            | Recovery actions                                                                                                                                                                 |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | E (remote): the next inference is blocked (`remote: consumed_home_semantic_revision_blocks_next_remote_inference`). Local: the next boundary recheck stops the Run; adoption of a response already in flight is decided by the authority guard, and no test yet races a local Unit correction with inference | E: the Run, including a writer that corrected its own read, stops before another inference and its Run details are hidden (`home_run_reads_survive_reindex_but_the_writer_is_invalidated_by_its_own_correction`) | E: derived Units on the old revision retire (`review_regressions: stale_ordinary_dependents_retire_without_ttl_and_release_live_capacity`, `review_regressions: derived_corrections_preserve_kind_and_require_derive_authority`). #118 candidates: P, as row 4                                                                                        | E: pause with cause `invalidated`, local and remote; the only action is `create_follow_up_task` (see [Typed recovery](#typed-recovery))                                          |
| 2   | as row 1                                                                                                                                                                                                                                                                                                     | as row 1                                                                                                                                                                                                         | E: conflicts stay visible to consolidation (`observation_consolidation_keeps_conflicts_and_recomputes_surviving_evidence`)                                                                                                                                                                                                                            | as row 1                                                                                                                                                                         |
| 3   | E: the attempt is interrupted and never accepted (`harness: revoked_authority_mid_stream_interrupts_and_stops_progress`, `remote: revocation_during_remote_inference_discards_response_and_cancel_closes_both_sides`)                                                                                        | E: pause; a revoked Binding stops even a deferred Run (ADR 0013; `remote: generated_home_lineage_is_rechecked_when_semantic_memory_is_disabled`)                                                                 | E: unchanged; every read rechecks current authority                                                                                                                                                                                                                                                                                                   | E: cause `authority` (remote) or the generic `execution authority denied` pause (local, ADR 0013); restore authority, then `retry_unchanged_binding`, or `create_follow_up_task` |
| 4   | E (remote): the response is rejected (`remote: source_deleted_during_remote_model_request_rejects_the_response`)                                                                                                                                                                                             | E: pause with `invalidated` (`remote: ordinary_native_remote_context_uses_home_units_and_deletion_stops_inference`)                                                                                              | E: dependents are withheld and never revived by derived jobs or restore (`durable_derived_jobs_refresh_questions_and_never_revive_deleted_sources`, `old_memory_archive_cannot_revive_deleted_corrected_or_dependent_bodies`). Physical purge is tracked separately (`purge: physical_purge_failure_is_bounded_fair_and_survives_adapter_recreation`) | E: `create_follow_up_task`; `memory_cleanup` on the Run management view reports physical cleanup apart from retrieval exclusion                                                  |
| 5   | E: an Ordered Run re-retrieves once on a stale reuse; a Legacy Run stops (`harness: conflicting_recheck_discards_the_ordered_cache_and_retrieves_once`, `conflicting_recheck_still_stops_a_legacy_run`)                                                                                                      | E: continues                                                                                                                                                                                                     | E: none                                                                                                                                                                                                                                                                                                                                               | E: `wait` while indexing or the backend is unavailable, then `retry_unchanged_binding` (`remote: semantic_retry_exhaustion_pauses_and_manual_resume_keeps_attempt_history`)      |
| 6   | E: unaffected                                                                                                                                                                                                                                                                                                | E: unaffected                                                                                                                                                                                                    | E: a dormant Unit stays evidence for derived content (`memory_decay: live_derived_support_reactivates_and_remains_evidence_even_when_dormant`)                                                                                                                                                                                                        | E: none required; `Reactivate` restores ordinary recall                                                                                                                          |
| 7   | P: as row 4                                                                                                                                                                                                                                                                                                  | P: as row 4                                                                                                                                                                                                      | P: pending candidates from the batch become `invalidated`; derived dependents are withheld                                                                                                                                                                                                                                                            | P: `create_follow_up_task`                                                                                                                                                       |

The `memory_decay:` test lives in
`server/src/apps/knowledge/repositories/memory_decay/tests.rs`.

### What no change does

- Aidash cannot recall information already disclosed outside Aidash. Retrieval
  exclusion, derived-content invalidation and physical cleanup are reported
  separately; none of them claims to erase an earlier disclosure.
- A change never lets a Run continue from the old text. An invalidated Run's
  Context Journal already contains the obsolete content, so continuing it with
  fresh context is not offered (P, see the decisions below).
- A follow-up Task carries only instructions the requester supplies. It has no
  parent or dependency link to the invalidated Run, so no invalidated context,
  approval or verification is copied, and completed effects are not replayed.

## Typed recovery

A Run stopped by its semantic context reports the exact cause and the actions
the current contract authorizes. `GET /api/runs/{id}/management` returns
`semantic_recovery` next to `semantic_reason` while the Run is neither cancelled
nor finished. The view carries no content, so it stays available when the Run's
content is hidden. On the Home, each entry of
`GET /api/tasks/{task}/remote-executions` reports the same value for a remote
Run as `semantic.recovery`, also while semantic memory is disabled, and omits it
once the remote Run is cancelled or finished. A retry counts as scheduled only
while the Run is active: a paused Run is never scheduled, so it offers
`retry_unchanged_binding` instead of `wait`. While a remote Run reports a
semantic cause, the Home's remote execution controls follow these actions:
resume is offered only with `retry_unchanged_binding`, and the follow-up form
only with `create_follow_up_task`.

| Cause                                                                                                                       | Actions                                            |
| --------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| `pending` or `unavailable` with a scheduled retry                                                                           | `wait`                                             |
| `pending` or `unavailable` without one, `retries_exhausted`, `authority`, `configuration`, `allowance`, `provider_contract` | `retry_unchanged_binding`, `create_follow_up_task` |
| `invalidated`, `context_budget`                                                                                             | `create_follow_up_task`                            |

- `wait`: the Run retries on its own schedule.
- `retry_unchanged_binding`: an authorized manual resume. It passes only if
  every pinned revision and current authority still hold; otherwise the Run
  pauses again at its next boundary. The resume endpoint still accepts an
  `invalidated` Run, which then pauses again with the generic authority pause
  and no inference.
- `create_follow_up_task`: a new Task with instructions the requester supplies.
  For a remote Run, the Home's follow-up endpoint creates it without parent or
  dependency links; for a local Run, create an ordinary Task.

A local Run whose memory read journal fails pauses with `invalidated` when a
consumed Unit was corrected, withdrawn, deleted or expired, and with
`configuration` when its memory participant or provider changed. Lost read
authority keeps the generic authority pause of ADR 0013, so a revocation is
never reported as a source change
(`server/src/apps/knowledge/repositories/memory_reads/tests.rs`). "Obtain fresh context and re-evaluate" is
not offered to a paused Run: an Ordered Run re-retrieves once only when a
reused retrieval goes stale before it is sent (ADR 0020).

## Impact inspection

`POST /api/workspaces/{workspace}/memory/units/impact` takes the provider, the
Bank and up to the pinned `max_candidates` Unit IDs, and needs `memory.read` on
that Bank. For each Unit it returns the revision a Run may still read
(`readable_revision`, `null` once the Unit is deleted, withdrawn, retired, past
its retention or without current support, even before the maintenance sweep),
the Runs that read it and the live derived Units that depend on it. A derived
Unit is live under the same checks as listing: visible (not deleted, stale or
withdrawn), within retention and with current support. The same view serves as
the preview before a change and as the post-change view: a Run read is `fenced`
once its revision is no longer readable.

- The view is content-free: identities, revisions, kinds, phase and control,
  never Unit text, subjects or Run content. The memory page shows it under
  each Unit as "Dependent Runs and derived memory". A cancelled or finished
  Run is still listed as a historical reader, but the page does not claim that
  a change stops it: it has no next boundary.
- A Run is listed only if its base disclosure holds for the requester (task
  visibility, `run.read` and `memory.read` on its participant). A derived Unit
  is listed only with `memory.read` on its Bank. Traversal continues through
  hidden Units, but nothing hidden is listed or counted.
- Bounds: at most 64 visible Runs per Unit from at most 256 Run read records,
  and the pinned `max_graph_visits` for derived content. `complete` is `false`
  when a bound stopped a scan; because hidden records also consume the scan,
  the flag is the only signal they can affect. Run reads are found through a
  lookup index that leads with the Unit
  (`knowledge/0029_memory_run_read_lookup`), so the scan bound also bounds the
  work.
- A Unit of another Bank answers exactly like an unknown ID (`null`, no Runs,
  no derived Units), so the view cannot probe identities outside the Bank
  (`impact_answers_a_foreign_unit_like_an_unknown_id`).
- Runs on other Nodes are not listed; the Home's remote status reports their
  `semantic.recovery`.
- The view is neither a lock nor a promise. Every mutation rechecks revisions
  and authority when it commits.

The tests are `impact_lists_visible_dependents_before_and_after_a_change` and
`impact_reports_only_live_units_and_fences_expired_reads`.

## Concurrency and restart

| Scenario                            | Supported outcome                                                                                                                                             | Test                                                                                 |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| A Run corrects memory it consumed   | The correction commits; the writer stops at its next boundary with `invalidated` and makes no further inference                                               | `home_run_reads_survive_reindex_but_the_writer_is_invalidated_by_its_own_correction` |
| Two concurrent writers on one Unit  | One compare-and-set winner; the loser gets a conflict, never a silent overwrite                                                                               | `concurrent_writers_on_one_unit_have_a_single_winner_and_fence_readers`              |
| Several readers of one Unit         | Every reader of the old revision is fenced by the winning change                                                                                              | same test                                                                            |
| Home restart after an invalidation  | The Run stays paused with the same cause and revision; no restart loop                                                                                        | `home_run_reads_survive_reindex_but_the_writer_is_invalidated_by_its_own_correction` |
| Manual resume of an invalidated Run | The worker's entry check withholds the Run and pauses it again without inference; this pause reads `execution authority denied`. P: reject the resume instead | same test                                                                            |
| Remote Execution node restart       | Not covered here; the remote semantic journal replays receipts (`remote: lost_semantic_reply_replays_receipt_without_a_second_embedding`)                     | P: an explicit Execution-node restart test                                           |

Evaluation hook for #122: the fixed-task evaluator (PR #213) is not merged.
When it is, these scenarios should report, per Run, the pause cause, the number
of worker attempts until the pause (continuity), the time from the source
change to the pause (latency), and whether a follow-up Task completed the
original goal (useful completion).
