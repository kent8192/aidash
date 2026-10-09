# Issue #155 Phase 4 handoff

Phases 1–3 supply the execution components and charts. Existing GCP Terraform,
VM runtime and lifecycle controller remain unchanged until the sequencing gate
opens. On 2026-10-09, PRs [#161](https://github.com/kent8192/aidash/pull/161),
[#162](https://github.com/kent8192/aidash/pull/162) and
[#163](https://github.com/kent8192/aidash/pull/163) were open. Recheck all three
before starting Phase 4 and incorporate their merged base in the authoritative
task worktree. Preserve Provider Credential, capability broker and GCIP resources
and carry their grants onto the designated Workload Identity principals.

Build the charts, Runner, installer, guard, observer and activity collector from
the trusted workflow ref and pin their images by digest. Deployed fork PR source
must never supply privileged code or chart templates. The separate application,
trusted DaemonSet and restricted Execution Pod namespaces are trust boundaries.
The installer has cluster-wide Cluster Node get/patch RBAC; the Runner can exec
guard Pods only in the dedicated trusted namespace.

## Post-drain idle observation

The lead revised the seal sequence on 2026-10-09 after the producer audit below:

1. Close edge admission.
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

Repeat this audit after incorporating the three gated PRs. Confirm no additional
producer can submit while both roles are stopped; if there is one, report to the
lead before implementing the seal. No Rust role behavior changed in Phases 1–3.

## Integration and verification still required

The approved Phase 4 design includes GKE Standard, Ubuntu N2 Environment pools,
the E2 system pool, Workload Identity, private Cluster Nodes and Cloud NAT,
retained disks, generation fencing, durable lifecycle locking, one active
Environment per kind, ephemeral LoadBalancer/DNS resume and static preview TLS
PV rebinding only after the old attachment is gone. The existing cloud APIs and
controller are not yet wired to the new charts. Preserve their current safety,
credential and activity checks when replacing VM lifecycle operations.

Local kind verification with upstream gVisor passed isolation, freeze, guest
process capacity and host task limits. It does not prove GKE Cluster Node loss,
Spot preemption, persistent disk reattachment, preview disk rebinding, IAM or
resume/stop/destroy behavior. Real GKE drills and all cloud resource mutations
require the user's explicit authorization through the lead.
