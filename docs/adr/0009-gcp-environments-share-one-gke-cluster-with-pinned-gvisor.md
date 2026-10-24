# GCP Environments share one zonal GKE Standard cluster and run Execution Pods on pinned upstream gVisor

Decided on 2026-10-09 (#155). The single-VM K3s host is replaced by one zonal GKE Standard cluster that every Environment shares. Each Environment has its own namespace and node pool. Execution Pods run under upstream gVisor, which a privileged DaemonSet installs at a pinned version and checksum on Ubuntu node pools and registers as the `runsc` containerd runtime. GKE Sandbox is not used. PostgreSQL and NATS JetStream run in the cluster as single-replica StatefulSets on persistent disks. The goal is automatic recovery after one Cluster Node is lost, not uninterrupted service, because the Home recovery ledger, PostgreSQL, NATS and the Runner journal each live on a single-attach disk.

## Considered Options

- GKE Sandbox. Rejected after a spike on 2026-10-09: GKE's admission rewrites the gVisor mount hints of every emptyDir in a GKE Sandbox Pod to a sandbox-private mount, even when the Pod sets its own hints. The node guard can then neither read nor write an Execution Pod's files, and size limits are not enforced inside the sandbox. GKE's managed gVisor also lags upstream; it predates the upstream `force-shared` emptyDir annotation that would avoid the rewrite.
- Self-managed Kubernetes on Compute Engine with kubeadm. Kept as the fallback. It would cost always-on control-plane VMs and lose Workload Identity.
- GKE Autopilot. Rejected: it does not allow the privileged node guard and installer DaemonSets.
- Cloud SQL or AlloyDB for PostgreSQL. Rejected: neither offers PGroonga or pg_jsonschema, both forbid custom extensions, and Aidash migrations require both extensions.
- One cluster per Environment. Rejected: only one zonal cluster per billing account is exempt from the management fee.
- Uninterrupted service with replicated server and worker roles. Deferred until the Home recovery ledger stops depending on one local filesystem.

## Consequences

- The repository's Runner and sandbox image ran unchanged in the spike. The node guard writes the Sentry's `pids.max` itself, because GKE rejects a kubelet Pod pid limit below 1024. That host ceiling is a separate setting from the guest process limit, sized so the guest can reach its own limit: in the spike each guest process cost about two host tasks on top of about 43, so the old shared value of 128 capped guests at roughly 40 processes.
- Running a customized containerd runtime on GKE nodes is outside GKE's supported configuration. GKE's admission left Pods of the `runsc` RuntimeClass untouched in the spike; that is observed behavior, not a documented contract, and a GKE change could widen it. Node upgrades, auto-repair and recreation replace the node, and the installer DaemonSet installs gVisor again; in the spike a deleted node returned with gVisor in about five and a half minutes. Execution Pods must not schedule onto a node before the installer finishes, so the installer labels its node when gVisor is ready and the RuntimeClass requires that label. Node upgrades run in maintenance windows outside working hours; Execution Pods evicted by an upgrade become uncertain operations.
- GKE Sandbox no longer forces an always-on node. In the spike the cluster stayed healthy with every node pool at zero nodes and recovered when one pool was resized back. While any Environment runs, an untainted node must also run, because system Deployments such as kube-dns cannot schedule onto a tainted pr node pool.
- Workload Identity gives the server and the worker their own Google identities. Other Pods and the node service account hold no Aidash permissions. The shared-VM-identity rationale in ADR 0002 and ADR 0005 no longer applies; their conclusions remain as defense in depth.
- Losing a Cluster Node turns its in-flight operations into uncertain operations. No new termination-proof source is added.
- Spot Cluster Nodes are recreated automatically after preemption. The Runner never recreates an accepted operation, so infrastructure still never replays tool code.
- Idle stop drains workers instead of freezing them. Work that starts between the last idle observation and the drain can be interrupted even without `force`.
- Fork-PR previews share the cluster, isolated by a namespace, a tainted node pool, NetworkPolicy and the absence of IAM grants.
- k3s support is removed everywhere, including the acceptance profile and the release-candidate profile.
- Environment node pools use N2. T2D was stocked out in us-central1-a for over 30 minutes during the spike, and E2 exceeds the project's E2 vCPU quota; only the untainted system node uses E2.
