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

## GKE drill (2026-10-10)

With the user's authorization, an operator-run drill used the branch's
`infra/gcp/environments` Terraform (separate `terraform/drill-155` state prefix) in
the configured project, amd64 release images built by Cloud Build into the private
registry, and the real `infra/gcp/control/kube.py` helpers with both charts. The
GitHub workflow was not used: its Workload Identity Federation trusts only `main`.
Every drill resource, image and state object was deleted afterwards.

Verified on GKE 1.34 (`us-central1-a`, Ubuntu containerd N2 Spot pools):

- Deploy: claims, Secrets, both releases, the migration Job, writers, health
  (source SHA) and an ephemeral L4 LoadBalancer. HTTPS `/health` through the
  LoadBalancer returned the Environment node ID; HTTP redirected.
- Isolation: the upstream installer labelled both nodes; the Runner verified
  isolation and Python freeze: kernel `4.19.0-gvisor`, 112 guest children at
  `processes=128`, Sentry `host_tasks=512`, CPU 2, 2 GiB, no swap, disk limits,
  denied egress including `169.254.169.254`.
- Workload Identity: the server KSA receives the server account, the worker KSA
  the worker account, and the Runner and any other KSA only the pool's federated
  identity with no grants. Node attributes such as `kube-env` are not exposed.
- Seal and stop: a fresh, self-identified post-drain observation sealed the
  Environment; stop removed the LoadBalancer (no forwarding rules remained) and
  the node pools scaled to zero with all six disks retained and detached.
- Resume: the pools returned, disks reattached on new nodes, data survived, and a
  new LoadBalancer address served HTTPS.
- Cluster Node loss: deleting the node that ran the Runner, PostgreSQL, the edge
  and a running Execution Pod recovered automatically. Within about three
  minutes the Runner, PostgreSQL and edge ran on the surviving node with their
  disks reattached, the interrupted operation surfaced as `uncertain` without
  termination proof or replay, and a new operation completed on `4.19.0-gvisor`.
  The Spot pool recreated the deleted node.
- Preview TLS: `pr-1` bound `aidash-preview-tls`; after `pr-1` stopped, `pr-2`
  waited for the old attachment, rebound the volume and served the same Caddy
  state.
- Destroy: `pr-2`, `pr-1` and `test` deleted their namespaces, retained PVs and
  disks; only the preview disk remained, released.

The drill found one defect, fixed in this branch: GKE's default 100 GB
pd-balanced boot disks exceeded the default 250 GB regional SSD quota.

An operation submitted directly to the Runner keeps the Runner's activity busy
after it becomes `uncertain`, because only the application acknowledges results;
an idle seal therefore correctly deferred for the drill's `test` Environment.

Still unverified, because they need the trusted workflow on `main`, the
Cloudflare token, Google OAuth clients or Identity Platform:

- Cloudflare DNS publication, Caddy ACME issuance and `public_health`.
- The controller run from GitHub Actions: `get-credentials --dns-endpoint` as
  the deploy account, plan fences on real plans, the lifecycle lock and budgets.
- Server BYOK Secret Manager calls, worker KMS signing and GCIP tenant IAM with
  the drill's identities (the identities were verified, not the grants' use).
- Spot capacity unavailability.

[#137](https://github.com/kent8192/aidash/issues/137): the server and worker
identities are separate Google accounts on GKE and other Pods hold none.
[#149](https://github.com/kent8192/aidash/issues/149): Execution Pods cannot
reach the metadata server; application Pods reach only the GKE metadata server,
which serves their Workload Identity and hides node credentials and attributes.
