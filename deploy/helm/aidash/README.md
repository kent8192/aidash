# Aidash chart

The default chart runs the server, worker and frontend. Set `frontend.enabled:
false` when the application image serves the web bundle itself. Provision role
Secrets, PostgreSQL, JetStream, and an independently retained and initialized
Home recovery ledger before enabling native memory. See [orchestration](../../../docs/orchestration.md).

`image.digest` (`sha256:<hex>`) pins server and worker to
`<repository>@<digest>` and takes precedence over `image.tag`. `release.sourceSha`
(40 hex characters) annotates both Deployments with `aidash.run/source-sha`, so a
controller can tie a completed rollout to the deployed commit.

Server and worker have separate Kubernetes ServiceAccounts. Set
`server.serviceAccount.annotations` and `worker.serviceAccount.annotations` to
their respective `iam.gke.io/gcp-service-account` annotations. These annotations
grant no IAM permissions by themselves. Environment pool selection and taints go
in `environment.nodeSelector` and `environment.tolerations`; no Pod uses
`hostNetwork`. The default loss tolerations are 30 seconds. Disk detach/reattach
still determines recovery time.

With `memoryRecovery.existingClaim` or `capabilities.storage.existingClaim`,
required Pod affinity puts every server and worker on the same Cluster Node. The
selector includes both roles and each Pod itself, allowing the first Pod to
bootstrap the group. Both Deployments use `Recreate` so a replacement does not
surge onto a second machine with a shared ReadWriteOnce claim. This profile pauses
during replacement and disk recovery. Keep any additional affinity compatible
with this placement. `capabilities.storage.existingClaim` is mounted at
`/var/lib/aidash/capabilities`; the capability object store is its `objects/`
child, which the application creates and restricts to its own UID (a fresh
volume root belongs to root and cannot be chmodded by the application).

## Edge, settings and network

`trustedProxy.cidrs` sets the server's `AIDASH_AUTH_TRUSTED_PROXY_IPS`: socket
peers, given as exact IPs or CIDR networks such as the cluster Pod range, that may
supply a single `X-Real-IP`. Pair it with `backendIngress.podSelectors`, which
renders a NetworkPolicy so that only the listed same-namespace Pods (for example
`{aidash.run/edge: env}`) and this release's frontend reach the server on 8080;
any other Pod in that range then cannot claim a client address.

`providerCredentials.settings` takes the managed Provider Credential descriptor
`{fingerprint_key, store, broker}`, never key material. The chart validates it
(a Secret Manager Store with its BYOK project and Environment, the fingerprint key
referenced as `AIDASH_PROVIDER_FINGERPRINT_KEY`, and a broker whose audience is
that Environment), renders `{"provider_credentials": ...}` into a ConfigMap and
points `AIDASH_PROVIDER_CREDENTIAL_SETTINGS` at it. Pass the all-null descriptor
to override file settings with "no managed Store"; `null` renders nothing.
`gcip.settings` takes `{dashboard: {gcip: {...}}}` with only the managed public
GCIP keys; its `public_origin` must equal `node.endpoint`. It is rendered the same
way for `AIDASH_GCIP_SETTINGS`. Changing either restarts server and worker.

## Metadata server egress

On GKE every node pool runs `GKE_METADATA`, so a Pod that reaches the metadata
server (`169.254.169.254`) receives only its own Kubernetes ServiceAccount's
identity and never the node's credentials. The chart still closes that path for
Pods that have no use for it, so a compromised Pod holds no federated token
either. `metadataServerEgress.enabled` (default `true`) renders the NetworkPolicy
`<release>-aidash-metadata-egress`, which selects every Pod of the release
namespace that does **not** carry the label `aidash.run/metadata-server` and
admits all egress except `169.254.169.254/32` and `169.254.169.252/32`. Server and worker carry the label;
nothing else does, so a new workload is blocked until someone labels it on
purpose. With `execution.guard` or `execution.installer` enabled, the same policy
is rendered in their namespaces (`<release>-aidash-guard-metadata-egress`,
`<release>-aidash-installer-metadata-egress`). The companion
[environment chart](../../../infra/gcp/helm/environment/README.md) renders its own
policy for its Pods.

Upgrading a running release from a chart version without the label needs care.
Helm applies the NetworkPolicy before the Deployments, and the server and worker
Pods already running lack the label until their replacements roll out, so they
lose the metadata server (and with it Workload Identity token refresh) for the
length of that rollout. The GCP controller is unaffected: every upgrade it runs
(`deploy` and `reload` in `infra/gcp/control/kube.py`) renders server and worker
at zero replicas and starts labelled replacements afterwards. A direct
`helm upgrade` of a release with live writers must either scale server and worker
to zero first, or upgrade once with `metadataServerEgress.enabled=false` and
enable it in a second upgrade after the labelled Pods are Ready.

NetworkPolicy egress is additive across policies, so no other policy that selects
these Pods may grant egress. The chart's other policies are ingress-only, and
`scripts/test_helm_execution.py` fails if one gains an egress rule. The policy
covers IPv4 only, which is the verified GKE profile; it does not admit IPv6, so
dual-stack clusters need a reviewed variant. No Aidash Pod uses `hostNetwork`: a
host-network Pod bypasses NetworkPolicy and reaches the metadata server as the
node. Every workload template sets `hostNetwork: false`, and the same test fails
on anything else.

The verified profile is GKE Dataplane V2 (`ADVANCED_DATAPATH` in
`infra/gcp/environments/cluster.tf`). On GKE, `gke-metadata-server` DNATs
`169.254.169.254:80` to `169.254.169.252:988` in the nat PREROUTING chain.
Dataplane V2 enforces policy before the DNAT, so excepting `169.254.169.254/32`
is enough there. Calico enforces after the DNAT (see
[kubernetes/kubernetes#68078](https://github.com/kubernetes/kubernetes/issues/68078)),
so the DNAT target `169.254.169.252/32` is excepted as well. A Calico cluster is
not a verified profile.

Inventory of Pod kinds (`yes` means the Pod is on the allowlist):

| Pod kind                   | Chart             | Metadata server | Why                                                                                                                                                                                                                                                                                                            |
| -------------------------- | ----------------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| server                     | `aidash`          | yes             | Writes Provider Credentials to Secret Manager with its Workload Identity                                                                                                                                                                                                                                       |
| worker                     | `aidash`          | yes             | Signs with Cloud KMS with its Workload Identity                                                                                                                                                                                                                                                                |
| frontend (Nginx)           | `aidash`          | no              | Serves the web bundle; holds no Google identity                                                                                                                                                                                                                                                                |
| Runner                     | `aidash`          | no              | Talks to the Kubernetes API with its own ServiceAccount; no IAM grants                                                                                                                                                                                                                                         |
| node guard DaemonSet       | `aidash`          | no              | Local host state and `kubectl exec` from the Runner; no API token                                                                                                                                                                                                                                              |
| gVisor installer DaemonSet | `aidash`          | no              | Patches Cluster Node labels through the Kubernetes API                                                                                                                                                                                                                                                         |
| migration Job              | controller        | no              | Copies the server's Pod template, ServiceAccount and Workload Identity, then replaces the labels, so it fails closed. It talks only to PostgreSQL, NATS (JetStream, for `activation-provision`) and the recovery volume. Created outside Helm by `infra/gcp/control/kube.py`, so the chart tests cannot see it |
| Execution Pod              | `aidash` (Runner) | no              | Own namespace; the Runner denies all of its ingress and egress (`aidash-deny`)                                                                                                                                                                                                                                 |
| PostgreSQL, NATS           | `environment`     | no              | Retained state; no IAM grants                                                                                                                                                                                                                                                                                  |
| edge (Caddy and admission) | `environment`     | no              | Public traffic and Firebase helper proxy                                                                                                                                                                                                                                                                       |
| activity CronJob           | `environment`     | no              | Kubernetes API and the Runner only                                                                                                                                                                                                                                                                             |

### Verifying on GKE (needs cloud access; not run by CI)

Run this against a cluster that has `GKE_METADATA` on every pool, with the chart
installed in an Environment namespace `$NS` as release `$REL`. The probes are
throwaway Pods that use a curl image, so they do not depend on the (minimal)
application images. Set `$POOL` to the Environment's node pool name; add a
matching toleration to `--overrides` if the pool is tainted.

```sh
URL=http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/email
probe() { # probe <name> <service account> <extra label>
  kubectl -n "$NS" run "$1" --rm -i --restart=Never --image=curlimages/curl $3 \
    --overrides="{\"spec\":{\"serviceAccountName\":\"$2\",\"nodeSelector\":{\"cloud.google.com/gke-nodepool\":\"$POOL\"}}}" \
    -- curl -sS -m 5 -H 'Metadata-Flavor: Google' "$URL"
}

# 1. The policy exists and selects only unlabelled Pods.
kubectl -n "$NS" get networkpolicy "$REL-aidash-metadata-egress" -o yaml

# 2. A non-allowlisted Pod times out. Expect: curl: (28) ... timed out.
probe md-denied default ''

# 3. A Pod with the allowlist label and the server's (then the worker's)
#    ServiceAccount receives that account's Google identity.
#    Expect an address like ad...-server@<project>.iam.gserviceaccount.com.
probe md-server "$REL-aidash-server" '--labels=aidash.run/metadata-server=allowed'
probe md-worker "$REL-aidash-worker" '--labels=aidash.run/metadata-server=allowed'

# 4. The real server and worker are labelled, Ready, and unaffected.
kubectl -n "$NS" get pods -l 'aidash.run/metadata-server=allowed'
```

A denied probe that answers means the cluster does not enforce egress policy.
A probe that times out in step 3 means the label is missing from the Pod or a
different policy selects it. Finish by confirming a Provider Credential write
(server) and a signing request (worker) succeed.

The policy also constrains Pods that need the API server. On GKE Dataplane V2,
anetd rewrites an `ipBlock` that contains node addresses into host, remote-node and
kube-apiserver selectors, so a `0.0.0.0/0` with an `except` must be checked, not
assumed. After the apply, confirm each of these still works:

```sh
# 5. The installer DaemonSet labels its node (needs the API server).
kubectl -n "$REL-installer" rollout status ds/"$REL-execution-installer" --timeout=300s
kubectl get nodes -l aidash.run/gvisor   # the execution pool's nodes carry the label

# 6. The Runner creates an Execution Pod (needs the API server).
kubectl -n "$NS" rollout status deploy/"$REL-execution-runner" --timeout=300s
# Run any Python cell through the application, then:
kubectl -n "$REL-sandbox" get pods        # an Execution Pod appears and completes

# 7. The activity CronJob completes (reads the API server and patches its ConfigMap).
kubectl -n "$NS" create job --from=cronjob/env-environment-activity md-activity
kubectl -n "$NS" wait --for=condition=complete job/md-activity --timeout=120s
kubectl -n "$NS" delete job md-activity
```

If any of these fail while the exempt Pods work, the policy is the suspect: delete
`<release>-aidash-metadata-egress` and retry to confirm, then widen it.

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
no new sandbox is scheduled while the guard keeps watching live ones. Kubelet does
not recheck that label for a Pod already bound to the Cluster Node, so the installer
also clears the execute bits of `containerd-shim-runsc-v1`, which every sandbox
start executes; such a Pod fails to start until the verified runtime restores them.
It then waits up to `AIDASH_GVISOR_DRAIN_SECONDS` (3600) for every process running
an installed `runsc`, `gvisor_sentry` or shim to exit, confirming an empty scan once
more after a pause. A running Sentry keeps its old executable
inode, which the guard's `samefile` check would stop recognizing, so the installer
fails without replacing anything if sandboxes remain; it stays fenced and retries
on restart. The new shim is also written without execute bits, whatever the
archive order, and becomes executable only after every runtime file, the receipt
and containerd's configuration and restart are complete. Installers of releases
sharing a Cluster Node hold one host `flock` (`/run/lock/aidash-gvisor-installer.lock`)
from withdrawing admission until publishing it, and write through unique temporary
files. The
installer writes the runtime only under `/usr/local/bin`, so the chart rejects a
custom `execution.paths.runsc` or `gvisorBin` while it is enabled.

The release gets four separate namespaces: the application release namespace,
`<release>-sandbox` (Pod Security `restricted`), `<release>-guard` (the trusted
privileged guard DaemonSet only) and `<release>-installer` (the privileged
installer DaemonSet only; created only while the installer is enabled). Names can
be overridden with `execution.sandboxNamespace`, `trustedNamespace` and
`installerNamespace` but must remain distinct; when several Environments use the
same release name, set distinct namespace and RuntimeClass names. The installer
and runtime ClusterRoles and their bindings are named
`<release namespace>-<release>-execution-*`, so they never collide.
The trusted namespace must contain only the guard; never schedule workloads from
the deployed source there. Kubernetes RBAC cannot constrain exec to
DaemonSet-generated Pod names by label, so its namespace is the boundary.
The Runner gets `get`/`list` Pods and `create` `pods/exec` there, execution and
NetworkPolicy rights in its sandbox namespace, and `get` on its named RuntimeClass.
It has no rights in the installer namespace, so a compromised Runner cannot exec
into the installer, which mounts the host root. It cannot read Secrets or patch
Cluster Nodes. The guard has no API token.

The Runner has one replica, `Recreate` updates, and a ReadWriteOnce journal. It
requires Kubernetes 1.34 or later, because Execution Pods set pod-level
`spec.resources`; the chart refuses to render it on older clusters.
Set `execution.runner.journal.existingClaim` to reuse a retained journal, or
provide a storage class and size for its kept claim. Use a class with reclaim
`Retain` on GCP. Supply its bearer token using `execution.runner.existingSecret`
and `execution.runner.tokenKey`; never put it in Helm values.

With the Runner enabled the chart also renders the application's capability
profile (`<release>-aidash-capability-profile`) and points server and worker at it
through `AIDASH_CAPABILITY_PROFILE`, with `admission: true`. It shares the Runner's
guest/resource limits (without the Runner-only `host_tasks`), stores objects in
`capabilities.storage.existingClaim` (required), caps the operation and install
limits at `maximum_seconds`, and names the `<release>-execution-runner:8949`
endpoint, Execution Pod image, RuntimeClass and sandbox namespace. Server and
worker read `AIDASH_CORE_RUNNER_TOKEN` from the Runner's own Secret key.

The profile keeps guest `processes=128` (16 to 4096, the application capability
profile's range) and separate `host_tasks=512`.
The Sentry's own host threads share that `pids.max` with the probe's
`processes - 16` guest children, and only the 128/512 profile is verified, so
`host_tasks` must be at least `max(4 * processes, processes + 384)`.
`maximum_seconds` ranges from 30 (the admission probe's own request) to 600, the
node guard's limit for Python cells. `idle_seconds` is capped at 1800, the guard's
frozen-session deadline, and `output_bytes` at 8 MiB: stdout and PNG displays each
use that budget and are base64-encoded into one result file the guard caps at
24 MiB. `working_bytes` is capped at 1 GiB, the most the guard exports and the
application profile accepts. With
the Runner enabled the release name is limited to 46 characters so
`<release>-execution-runner` fits a 63-character DNS label. The guard
writes and verifies Sentry `pids.max`; `sandbox.py` enforces guest `RLIMIT_NPROC`.
Resource evidence includes both. The guard requires host PID access, containerd,
kubelet emptyDirs with `HostToContainer` propagation, its own retained host state,
and the runtime executables for `samefile` checks. They are seen through directory
mounts, so an installer replacement is visible without restarting the guard: the
directory containing `runsc` is mounted read-only at `/run/aidash-host-runtime`, and
`gvisorBin` at its own path. Every `execution.paths.sentryBinaries` entry must be beside
`runsc` or under `gvisorBin`, and the list must include `<gvisorBin>/gvisor_sentry`:
the pinned runtime runs a split Sentry, and the guard kills only a PID it recognizes.
The Runner requires the guard (`execution.guard.enabled`).
`execution.paths` configures the runtime root, executable paths and cgroup root.
The guard state defaults to `/var/lib/aidash-node-guard/<release>`: each guard holds
an exclusive `watch.lock`, so releases sharing a Cluster Node must not share it.
Each guard migrates the shared legacy `/var/lib/aidash-node-guard` records: a live
record moves only after its container validates against this release's namespace,
while a terminal tombstone is copied without one, preserving termination proof.

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
