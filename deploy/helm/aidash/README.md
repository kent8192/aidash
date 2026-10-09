# Aidash chart

The default chart runs the server, worker and frontend. Provision role Secrets,
PostgreSQL, JetStream, and an independently retained and initialized Home recovery
ledger before enabling native memory. See [orchestration](../../../docs/orchestration.md).

Server and worker have separate Kubernetes ServiceAccounts. Set
`server.serviceAccount.annotations` and `worker.serviceAccount.annotations` to
their respective `iam.gke.io/gcp-service-account` annotations. These annotations
grant no IAM permissions by themselves. Environment pool selection and taints go
in `environment.nodeSelector` and `environment.tolerations`; no Pod uses
`hostNetwork`. The default loss tolerations are 30 seconds. Disk detach/reattach
still determines recovery time.

With `memoryRecovery.existingClaim`, required Pod affinity puts every server and
worker on the same Cluster Node. The selector includes both roles and each Pod
itself, allowing the first Pod to bootstrap the group. Both Deployments use
`Recreate` so a replacement does not surge onto a second machine with the shared
ReadWriteOnce ledger. This profile pauses during replacement and disk recovery.
Keep any additional affinity compatible with this placement.

## Optional execution components

Enable `execution.createNamespaces`, `execution.runtimeClass.create`,
`execution.installer.enabled`, `execution.guard.enabled` and
`execution.runner.enabled` for the complete profile. Every component is disabled
by default. `execution.sandboxImage` must contain an immutable digest. Build it
with `runner/Dockerfile`. Build the trusted image with:

```sh
docker buildx build -f runner/control.Dockerfile -t your-registry/aidash-runner:0.1.0 runner
```

Set **separate** `execution.runner.image`, `execution.guard.image` and
`execution.installer.image` values. All three must be built from the trusted
workflow ref, including for fork PR Environments. Never build their code, their
Dockerfile or their chart from deployed PR source. Only the Execution Pod image
may contain that source, using its trusted build recipe. Production controllers
must pin all trusted images by digest: the chart refuses to render an enabled
installer, guard or Runner whose image does not end in `@sha256:<digest>`.
Installer node patch permissions are cluster-wide, so granting them to
source-built code would cross Environment boundaries.

The only verified GCP profile is GKE Standard on Ubuntu/containerd N2 pools with
upstream gVisor; GKE Sandbox and Autopilot are excluded. The installer checks the
release archive SHA-256, validates all archive paths and regular files, writes
every binary (including `gvisor-bin`), verifies a receipt against installed file
hashes, registers containerd's version-2 `runsc` runtime, and restarts containerd
through PID 1 only after configuration changes. It publishes
`aidash.run/gvisor=20260921.0` after verification; the RuntimeClass requires it, so
this admission label decides where new sandboxes start. It also sets
`aidash.run/gvisor-installed=true`, which places the guard and is never withdrawn
once a runtime has been verified. Its ServiceAccount has only cluster-wide `get`/`patch` on Cluster Nodes.

The installer is privileged and rewrites host binaries, so the chart refuses to
render it without a global or Environment `nodeSelector`; select only the dedicated
execution pool. Before replacing a runtime it withdraws only the admission label, so
no new sandbox starts while the guard keeps watching live ones, then waits up to
`AIDASH_GVISOR_DRAIN_SECONDS` (3600) for every process running an installed
`runsc` or `gvisor_sentry` to exit. A running Sentry keeps its old executable
inode, which the guard's `samefile` check would stop recognizing, so the installer
fails without replacing anything if sandboxes remain; it retries on restart. The
installer writes the runtime only under `/usr/local/bin`, so the chart rejects a
custom `execution.paths.runsc` or `gvisorBin` while it is enabled.

The release gets three separate namespaces: the application release namespace,
`<release>-sandbox` (Pod Security `restricted`), and `<release>-guard` (trusted
privileged DaemonSets only). Names can be overridden but must remain distinct.
The trusted namespace must contain only the guard and installer; never schedule
workloads from the deployed source there. Kubernetes RBAC cannot constrain exec
to DaemonSet-generated Pod names by label, so its namespace is the boundary.
The Runner gets `get`/`list` Pods and `create` `pods/exec` there, execution and
NetworkPolicy rights in its sandbox namespace, and `get` on its named RuntimeClass.
It cannot read Secrets or patch Cluster Nodes. The guard has no API token.

The Runner has one replica, `Recreate` updates, and a ReadWriteOnce journal.
Set `execution.runner.journal.existingClaim` to reuse a retained journal, or
provide a storage class and size for its kept claim. Use a class with reclaim
`Retain` on GCP. Supply its bearer token using `execution.runner.existingSecret`
and `execution.runner.tokenKey`; never put it in Helm values. Configure the
application's own capability profile with the same guest/resource limits and
the `<release>-execution-runner:8949` endpoint. The chart's Runner profile does
not provision the application's retained object store or change its admission
configuration; those remain operator-provisioned state.

The profile keeps guest `processes=128` (minimum 8: the admission probe lowers its
own hard `RLIMIT_NPROC` to 8 and cannot raise it) and separate `host_tasks=512`, which
must exceed `processes` because guest processes are host tasks of the Sentry.
`maximum_seconds` is capped at 600, the node guard's limit for Python cells. With
the Runner enabled the release name is limited to 46 characters so
`<release>-execution-runner` fits a 63-character DNS label. The guard
writes and verifies Sentry `pids.max`; `sandbox.py` enforces guest `RLIMIT_NPROC`.
Resource evidence includes both. The guard requires host PID access, containerd,
kubelet emptyDirs with `HostToContainer` propagation, its own retained host state,
and the host executable paths mounted at the same paths for `samefile` checks.
`execution.paths` configures the runtime root, executable paths and cgroup root.
The guard state defaults to `/var/lib/aidash-node-guard/<release>`: each guard holds
an exclusive `watch.lock`, so releases sharing a Cluster Node must not share it.

The Runner selects the guard on the Execution Pod's recorded Cluster Node and
uses `kubectl exec -i` with the existing stdin/stdout JSON protocol. Old journals
without a Cluster Node/image binding recover it only from a matching live Pod
UID. An absent original Cluster Node, unavailable guard, or replaced Pod never
supplies termination proof; the operation stays uncertain through the existing
recovery path. Accepted code is never replayed by infrastructure.

For disposable local verification of these components, run
`python3 scripts/verify-execution-chart.py --name aidash-155-local --directory .ignore/issue155/local`.
It uses a private kubeconfig, a new kind cluster and upstream gVisor, and deletes
only that cluster after saving health/resource evidence. ARM's archive checksum
is used only for local verification; it does not establish a GKE ARM profile.
