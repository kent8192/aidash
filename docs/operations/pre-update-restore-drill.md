# Pre-update Backup restore drill

Run this before production activation, then quarterly, and after a material change
to backup selection, storage, the Home ledger or restoration. Acceptance needs an
actual supervised run; offline tests and Terraform configuration are not its
evidence. Production is not deployed yet, so the first run is pending in
[#562](https://github.com/kent8192/aidash/issues/562), together with the
lost-ledger drill.

Use a quiesced Pre-update Backup and its exact update entry. Keep that environment
sealed: the supported baseline has no `writers_started` marker. A failed update
that already resumed writers is repaired forward. The daily backup is a last
resort whose one-Node continuity target remains conditional on #433 (ADR 0082).

The script creates a new `aidash-drill-<random-id>` GCP project per drill through
Terraform. Its private Standard GKE cluster has no production VPC connection or
NAT. The source project receives only an additive cross-project service-agent
binding and a Restore Channel; cleanup removes both. The destination's restore
plan uses `MERGE_SKIP_ON_CONFLICT`, so each Restore can apply resource filters.
Only PVCs and ConfigMaps are included. Secrets, the exact ledger claim, all Pods
and workload controllers are explicitly excluded. No restored application starts
until a person installs the sealed release.

This uses Google's [cross-project restore procedure](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/how-to/cross-project-restores)
and [Restore filters](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/reference/rest/v1/projects.locations.restorePlans.restores#Filter).
The Restore Channel belongs to the backup's project and region. A successful
Restore operation is a data restoration milestone; application acceptance follows.

1. Identify who is running the drill, release SHA and image digests. Export the current update
   entry to `private/drill-entry.json` and identify the production lifecycle state
   bucket; it must name `pre_update_backup` and have
   no `writers_started`. Record the seal time, last acknowledged writes and exact
   fixture contents/high-water marks for every Node database, outbox, JetStream,
   tasks, Runs, artifacts, source memory and semantic projections. Record the
   current independently retained Home ledger's identity, epoch/revisions and
   digest. Do not copy credential values into evidence.
2. Confirm on the source cluster that the ProtectedApplication selects unlabelled
   resources and excludes the ledger claim/volume. The script checks every page
   of the completed Backup's volume inventory, its source cluster, application
   scope and successful volume data; an absent or incomplete inventory fails.
3. Obtain cloud access to create a project under the drill folder, attach its
   billing account, enable the listed APIs, create GKE, and manage Backup for GKE
   and the one source-project IAM member/Restore Channel. Use Application Default
   Credentials for Terraform and that person's `gcloud` account for the
   Backup API. No deploy service account is granted restore permission. Supply
   IDs and the client's public IPv4 `/32` in `private/drill-settings.json`:

   ```json
   {
     "billing_account": "<billing-account-id>",
     "folder_id": "<drill-folder-id>",
     "region": "us-central1",
     "client_cidr": "<client-ip>/32",
     "namespace": "aidash-production",
     "ledger_claim": "memory-recovery",
     "source_cluster": "projects/<source-project>/locations/us-central1/clusters/<cluster>"
   }
   ```

4. Start the recovery clock before provisioning the replacement project. Choose a
   new directory; keep its Terraform state and receipt together, with restricted
   access. `prepare` reads metadata and writes local files; `apply` creates the
   reviewed isolated infrastructure. Each later command checks the saved inputs.

   ```sh
   python3 infra/gcp/restore_drill.py prepare --directory private/drill-2026-10 \
     --entry private/drill-entry.json --settings private/drill-settings.json \
     --state-bucket <production-lifecycle-state-bucket>
   python3 infra/gcp/restore_drill.py apply --directory private/drill-2026-10
   python3 infra/gcp/restore_drill.py restore --directory private/drill-2026-10
   ```

   `prepare` and the final pre-Restore check reread `lifecycle/state.json` from
   that bucket using `gcloud storage cat`. A changed baseline or a live
   `writers_started` marker refuses the restore. Keep the source sealed for the
   entire drill; a saved export alone cannot establish current quiescence.

   A retry reuses the same Restore ID and verifies its baseline and filters.
   Inspect failed operations; success does not set `application_verified`.

5. Use an isolated `KUBECONFIG` under that directory when obtaining the target
   cluster credentials. Check that there are no restored Secrets, application
   Pods/controllers or ledger PVC. Retain the Restore and volume-restore
   metadata, target PVC inventory and these observations as raw evidence.
6. Copy the pinned release images into the target project's `restore-drill`
   Artifact Registry repository, preserving and verifying their content digests.
   Generate target references from the sealed release receipt first:

   ```sh
   python3 infra/gcp/restore_drill.py images --directory private/drill-2026-10 \
     --release private/sealed-release.json
   ```

   `image-map.json` records each source/target pair; copy each with `gcrane cp`,
   then compare `gcrane digest <target-reference>` with its recorded SHA-256.
   Use only the generated `release.json` when rendering release values, including
   app, control, frontend and installer images. Preserve the sealed release SHA;
   never feed the source receipt back into `app_values` or other chart values.
   Retain the copy/digest evidence and rendered workload image inventory. The
   receipt pins hashes of both generated files and the source commit. The images
   step is idempotent for that same release; switching releases requires a new
   drill. `record` refuses missing/changed files, another revision, and incomplete
   digest sets.
   Install the release's pinned upstream gVisor and compatible storage/runtime
   prerequisites on the Ubuntu nodes. Re-provision Secrets from Secret Manager
   into the target using cloud access; never restore backed-up Secrets or
   grant the target nodes production Secret Manager access. Replace production
   peer/provider destinations with owned, isolated fixtures before starting any
   workload. The drill nodes carry the production placement label and enable
   Stateful HA, so the production selectors and HA custom resources are supported.
   Dataplane V2 enforces the production charts' NetworkPolicies; verify denied
   sandbox, metadata, backend and database traffic before accepting the drill.
   Terraform also enables Workload Identity and creates target-only server/worker
   accounts. Read `terraform output -json workload_accounts` and override the
   chart's server/worker KSA annotations with those accounts before rendering.
   Their only KSA members are `app-aidash-server` and `app-aidash-worker` in the
   configured namespace; preserve the `app` Helm release name or review matching
   IAM changes. Never reuse a production GSA annotation.

   Create an isolated GCIP tenant in the drill project's Identity Platform,
   configure only owned test users/providers and the isolated callback origin,
   and replace `dashboard.gcip` project/tenant/API-key/callback settings before
   starting the server. The drill server's custom role permits only
   `firebaseauth.users.get` in the drill project; it has no production GCIP or
   Secret Manager access. Through the existing administrative restore/bootstrap
   procedure, bind the isolated identities to the retained fixture Subjects and
   policy without granting test credentials access to production. Verify ADC as
   the server KSA, account-status lookup for an enabled and then disabled/revoked
   test user, and refusal from the worker KSA before application acceptance.
   Keep admission closed and the server/worker stopped while inspecting
   restored configuration and transaction state.

7. Attach an independently protected **current** copy of each Home ledger to its
   matching restored Home. Verify the recorded identity, epochs and revisions;
   never run ledger `init` against restored nonempty data. Do not substitute an
   empty directory when the retained ledger is missing. Missing/inconsistent
   ledger continuity fails the drill. #434 separately supplies off-disk
   protection; this procedure does not claim that it exists.
8. A person verifies baseline high-water marks and exact contents; authorized
   reads/writes and denied/cross-Tenant/revoked reads; safe Run recovery with no
   duplicate external effects; event/outbox replay; semantic search and exclusion
   of deleted/revoked sources. Reconcile unsafe effects explicitly. Reopen only
   through the normal serving/admission gates. Stop the recovery clock after all
   checks pass. Also exercise the PR 1 in-place restore conflict policy on this
   disposable environment with an independently retained ledger marker and
   compare its identity/content before and after. Cross-project omission alone
   does not prove `DELETE_AND_RESTORE` preserves an existing claim.
9. Write `private/drill-2026-10/verification.json` with these fields. Every check
   is a Boolean backed by the raw `report`; use the full 40-character release
   commit SHA, SHA-256 image digests, and nonblank person/report strings. The
   fault time must be no later than the receipt's preparation time, and verified
   time no earlier than its restore completion. Begin with `false` and change only
   after observation. `baseline_high_water` records the update-start RPO check.

   ```json
   {
     "performed_by": "<name>",
     "revision": "<release-sha>",
     "image_digests": ["sha256:<digest>"],
     "fault_at": "2026-10-23T00:00:00+00:00",
     "verified_at": "2026-10-23T01:00:00+00:00",
     "report": "<retained-raw-evidence-location>",
     "checks": {
       "baseline_high_water": false,
       "ledger_preserved": false,
       "secrets_reprovisioned": false,
       "authorized_read_write": false,
       "denied_access": false,
       "execution_recovery": false,
       "no_duplicate_effects": false,
       "event_outbox_replay": false,
       "semantic_search": false,
       "deleted_sources_absent": false
     }
   }
   ```

   ```sh
   python3 infra/gcp/restore_drill.py record --directory private/drill-2026-10 \
     --evidence private/drill-2026-10/verification.json
   python3 infra/gcp/restore_drill.py destroy --directory private/drill-2026-10
   ```

   `record` rereads the live production update entry again; resumed writers or
   another baseline refuse acceptance. A new record attempt clears the previous
   acceptance flag until all checks succeed. Acceptance requires every check,
   the complete pinned release digest set, and an elapsed RTO at most two hours. A failed
   check or missed target remains a failed result. Always clean up after retaining
   evidence, including on failure: `destroy` removes only this drill's Restore,
   Terraform-managed channel/binding and dedicated project. If cleanup fails,
   retain its state and retry; do not delete the receipt or import an existing
   project. Confirm the target project is scheduled for deletion and the source
   channel/member are absent. Keep receipt, input hashes, raw reports, cleanup
   evidence and the next quarterly due date outside the disposable project.
