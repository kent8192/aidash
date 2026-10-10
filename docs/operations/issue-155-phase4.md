# Issue #155 Phase 4 handoff

Phases 1–3 supplied the execution components and charts. Phase 4 replaces the
per-Environment VM with the shared GKE Standard cluster described in the
[GCP Environments guide](../../infra/gcp/README.md). The work started from the
merged base of PRs [#161](https://github.com/kent8192/aidash/pull/161),
[#162](https://github.com/kent8192/aidash/pull/162) and
[#163](https://github.com/kent8192/aidash/pull/163). Provider Credential,
capability broker and GCIP resources are preserved; their grants moved to the
server and worker Workload Identity principals.

Build the charts, Runner, installer, guard, observer and activity collector from
the trusted workflow ref and pin their images by digest. Deployed fork PR source
must never supply privileged code or chart templates. The separate application,
trusted DaemonSet and restricted Execution Pod namespaces are trust boundaries.
The installer has cluster-wide Cluster Node get/patch RBAC; the Runner can exec
guard Pods only in the dedicated trusted namespace.

## Post-drain idle observation

The lead revised the seal sequence on 2026-10-09 after the producer audit below:

1. Close edge admission: record `closed` in the controller-owned ConfigMap
   `env-environment-admission`, then close the live edge (when an edge Pod is
   live). Opening is the reverse order. Every edge reads this state at start, so a
   restarted edge keeps the last decision and a partial failure stays closed. An
   Environment without any live Pod has nothing to drain and is already sealed.
2. Scale **server and worker** to zero and wait for their graceful drain to finish.
3. Keep the Runner, edge, PostgreSQL, NATS and observer running. Obtain fresh
   database work, edge activity and authenticated Runner `/v1/activity` evidence.
   Minute activity snapshots alone do not replace this synchronous observation.
4. If any work is active or any source is unavailable, restore server, worker and
   admission and defer the stop. A stale snapshot cannot authorize stopping.
5. Only after an idle verdict, stop the Runner and remaining workloads, remove
   the LoadBalancer Service and DNS, and scale the Environment pool to zero.
   Keep PVCs. Stop the system pool only when every Environment is stopped.

Work started between the last idle observation and the drain can be interrupted
**even without `force`**. Neither drain completion nor Kubernetes eviction is
termination proof. Operations without the guard's existing proof remain
uncertain and accepted code is never replayed by infrastructure. Do not stop the
Runner before observing it, and do not add a journal-PVC observer as a substitute.

The source audit at the Phase 1–3 base found both Runner submission paths inside
server/worker processes:

- `server/src/apps/execution/services/node_commands.rs` runs the server via
  `bootstrap::serve(settings, false)` and the worker through its own runtime.
- `server/src/bootstrap/compatibility.rs` starts `RuntimeTasks` for the server
  even with zero agent workers. `server/src/bootstrap.rs` starts capability
  background jobs whenever `AIDASH_CAPABILITY_PROFILE` is set, independently of
  `worker_count`. Stopping only worker Pods therefore leaves producers alive.
- `crates/aidash-runtime/src/capabilities.rs` owns reconciliation and reference
  extraction loops. Application `capabilities/reconciliation.rs` submits and
  starts operations; `capabilities/references.rs` submits extraction operations.
  Their HTTP transport is composed in server repositories through
  `bootstrap::operation_runner`; federation and schedulers run within these roles.
- The observer and activity CronJob only read database/edge/Runner activity and
  patch their named ConfigMap. The Runner's existing startup recovery threads
  may continue accepted work; `/v1/activity` counts pending and unproven records
  as busy, including old journals. The guard watchdog supplies its existing
  termination evidence and does not submit operations.

The audit was repeated on the merged #161/#162/#163 base: every Runner submission
path is inside the server/worker processes, and the Credential Broker cannot mint
or submit without them. Scaling both roles to zero stops all producers. Repeat it
whenever a new background producer is added.

## Implemented

- Terraform: shared network, Cloud NAT, node service account, zonal cluster
  `aidash` with Dataplane V2, Workload Identity and private nodes, the E2 system
  pool (one node iff any Environment runs), per-Environment Ubuntu N2 pools with
  label/taint, server/worker service accounts and the preview TLS disk. VM,
  firewall, runtime service account, startup script and bundle resources are
  removed; deploy no longer has OS Login or IAP grants.
- Release: eight digest-pinned images; `app`, `postgres` and `sandbox` from the
  deployed source, `observer`, `control` and `edge` only from the trusted
  checkout, `nats` and `caddy` from reviewed upstream digests.
- Charts: digest image values, retained claims, capability storage, provider and
  GCIP settings, trusted-proxy CIDR and backend NetworkPolicy; the controller owns
  the StorageClass and preview PV.
- Controller: Kubernetes/Helm deploy, the post-drain seal above, stop/resume,
  destroy with retained PV cleanup, preview PV rebinding, GCIP quiesce, broker
  descriptor updates and plan fences for the cluster, preview disk and node pools.
  Generation fencing, budgets, the durable lock and CI/source checks are unchanged.
- The VM runtime (`infra/gcp/runtime/`), its tests and the VM host references outside
  history records are removed.

## Verification still required on real GKE

Local verification covers provider-mocked Terraform, controller regressions with
patched Kubernetes/Helm helpers, Helm rendering, the edge admission container and
kind with upstream gVisor (isolation, freeze, guest process capacity and host task
limits). It does not prove:

- Cluster Node loss: Pods reschedule, retained disks reattach and in-flight
  operations surface as uncertain without replay.
- Spot preemption and Spot capacity unavailability for test/PR pools.
- Persistent disk reattachment of PostgreSQL, NATS, journal, Home ledger and
  capability claims across stop/resume.
- Preview TLS PV rebinding between `pr-N` namespaces, including the
  VolumeAttachment wait.
- Workload Identity and IAM: server BYOK Create/Manage and GCIP tenant read,
  worker KMS signing, and denial for the node service account and other Pods.
- Re-check of [#137](https://github.com/kent8192/aidash/issues/137) and
  [#149](https://github.com/kent8192/aidash/issues/149) behavior on GKE.
- Full create, idle stop, resume, PR replacement/close and destroy cycles.

Real GKE drills and all cloud resource mutations require the user's explicit
authorization through the lead.
