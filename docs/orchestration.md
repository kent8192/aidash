# Kubernetes and k3s

The Helm chart deploys one stable Aidash node as separate server and worker
Deployments. Install the same chart twice for two independent nodes. Each node
needs its own PostgreSQL database and node ID; every replica of that node uses
the same database, identity, API credentials and pair-specific peer secrets.
Pod names and replica counts never become node identities.

## Build and deploy

Build and publish both chart images using your registry workflow. The server and
worker use the default runtime target; the dashboard uses the separate `frontend`
target, which packages the built dashboard in an unprivileged NGINX image. The
runtime image includes the dashboard assets and runs as UID 10001 without build
tools. Both ARM64 and AMD64 builds use their native Rust target. Use immutable
image tags for each rollout; the chart does not publish images.

```sh
docker build -t your-registry/aidash:0.1.0 .
docker push your-registry/aidash:0.1.0
docker build --target frontend -t your-registry/aidash-frontend:0.1.0 .
docker push your-registry/aidash-frontend:0.1.0
```

Provision a namespace and an existing Secret for each node. The Secret supplies
`DATABASE_URL`, `NATS_URL`, `AIDASH_API_TOKEN`, and each required `AIDASH_SECRET_*`
variable. Give both ends of a peer link the same strong pair credential. Model
and embedding credentials must exist on every server and worker that can use
them. Keep actual secret values outside Helm values and Git. A projected
ServiceAccount token is used only by the server's optional Kubernetes observer.

```sh
helm upgrade --install node-a deploy/helm/aidash --namespace aidash \
  --set node.id=aidash://node-a --set existingSecret=node-a-secrets \
  --set image.repository=your-registry/aidash --set image.tag=0.1.0 \
  --set frontend.image.repository=your-registry/aidash-frontend \
  --set frontend.image.tag=0.1.0 --wait
helm upgrade --install node-b deploy/helm/aidash --namespace aidash \
  --set node.id=aidash://node-b --set existingSecret=node-b-secrets \
  --set image.repository=your-registry/aidash --set image.tag=0.1.0 \
  --set frontend.image.repository=your-registry/aidash-frontend \
  --set frontend.image.tag=0.1.0 --wait
```

Register reciprocal peers using `http://node-a-aidash.aidash.svc:8080` and
`http://node-b-aidash.aidash.svc:8080`. For independent clusters, configure a
reachable TLS endpoint in `node.endpoint` and your ingress/load balancer. An
external endpoint must stay stable across Pod replacements. The default Service
is cluster-local; probes on port 8081 are not exposed through that Service.

## State and dependencies

Aidash Pods are stateless and run with a read-only root filesystem. PostgreSQL
retains workspaces, task/run IDs, journals, artifacts, leases, transaction
decisions, policy and semantic source state. Use persistent volumes or a managed
database with durable storage, tested backups, and a retention policy independent
of the Aidash Helm release. Concurrent startup serializes migration execution.
Back up the database before an upgrade; schema rollback requires the migration's
documented preconditions, including resolving pending atomic transactions.

Aidash migrations require PostgreSQL 17 with `pg_jsonschema` 0.3.4 installed and
the extension created in each Aidash database before the application role runs
migrations. A database server administrator must install the extension files and
run this command as a PostgreSQL superuser (the extension declares
`superuser = true`):

```sql
CREATE EXTENSION pg_jsonschema WITH SCHEMA public;
```

The migration calls `public.jsonschema_is_valid(json)` directly and fails if the
extension is unavailable; it does not substitute partial JSON shape checks.
Ensure the Aidash database role can execute that public function. The local
PostgreSQL image installs the pinned extension package and initializes
`template1` plus `aidash_a`; `aidash_b` and `aidash_test` inherit it from
`template1`. The init script runs only for an empty data directory. For an
existing local volume, a server administrator must create the extension in
`template1` and every existing Aidash database before applying this migration.
The Helm chart uses externally managed databases, so provision this prerequisite
on those databases separately.

NATS must enable JetStream and persist its storage directory. Its connection URL
comes from the Secret. A broker outage leaves a durable PostgreSQL outbox;
readiness depends on PostgreSQL so agents can continue durable work during that
outage. Configure authentication/TLS for both backends according to your cluster
boundary. Size PostgreSQL connection capacity for all server/worker replicas and
their isolated recovery pools; do not assume a fixed total connection budget.

Semantic memory uses Qdrant 1.19.1 with persistent `/qdrant/storage`. Configure
its endpoint, secret reference and the explicitly selected embedding provider in
the workspace semantic index. PostgreSQL remains the source of authority and
holds retry/deletion records; a Qdrant outage is visible and retries are durable.
The chart intentionally connects to operator-managed dependencies. The acceptance
runner supplies disposable StatefulSets and PVCs for PostgreSQL, JetStream and
Qdrant; its fixture passwords and unprotected network are local-test settings.

## Scale, update and stop

Set `server.replicas` and `worker.replicas` through Helm/GitOps. Each worker Pod
runs four concurrent execution loops. Rolling updates allow one surge Pod and
keep existing ready replicas until replacements become available. Configure
resources, node selectors, affinity, tolerations and image pull secrets through
the chart values. Replica count zero stops that role while retaining its DB.
Scale both roles to zero to stop a node; scale them back up to resume. Removing
the chart leaves externally managed databases and volumes intact.

The runtime handles SIGINT and SIGTERM. It stops claiming new work, marks probe
readiness unavailable, drains current worker steps and HTTP requests for up to
20 seconds, then cancels remaining tasks. The chart gives it at least 25 seconds
before forced termination (30 by default). Long streams or effects may exceed
that budget; persisted leases, fencing and invocation replay rules determine
recovery. A shutdown is never treated as evidence that an external effect did
not occur. Unsafe effects pause for reconciliation; idempotent effects reuse
their durable key. SIGKILL bypasses draining and exercises the same recovery.

Port 8081 provides `/live` for process liveness and `/ready` for database readiness
and draining. A startup probe allows migrations and initial connections to finish
before liveness checks begin. Unavailable PostgreSQL removes a Pod from service
without putting it in a liveness-driven restart loop. A stopped background
service terminates the process so the Deployment can replace it.

## Observe

The dashboard's Deployment page shows desired, ready, updated and available
replicas, controller generation, Pod readiness/restarts, waiting/termination
reasons and current Kubernetes events. All controls are available in English and
Japanese; native Kubernetes diagnostic messages retain their original text.
An observation failure hides old successful observations and shows an error.
The page remains available while an atomic transaction blocks ordinary reads.

The observer has namespaced `list` access only to Deployments, Pods and Events,
and returns resources for its configured Helm release. Only Aidash operators can
read `/api/deployment`. It cannot read Secrets or mutate workloads. Do not share
the namespace across mutually untrusted operators, since Kubernetes list RBAC
cannot restrict objects by label. Set `observability.enabled=false` to remove
that Role/RoleBinding and token mount. Observation is disabled outside a
configured Kubernetes namespace. Tokens are reread on each request for rotation.

## Acceptance

For a complete disposable run, use `bash scripts/test-cluster.sh kubernetes` or
`bash scripts/test-cluster.sh k3s`. These commands verify pinned tool checksums,
create a private kubeconfig, build/import the image, run browser checks and remove
the cluster. CI requires both distributions. They need Docker, Python, curl,
Node dependencies and an installed Playwright Chromium.

To use a cluster you have already created, build and import the image, then run:

```sh
python3 scripts/cluster_acceptance.py --kubeconfig /path/to/private.kubeconfig \
  --distribution kubernetes --image aidash:orchestration-local
python3 scripts/cluster_acceptance.py --kubeconfig /path/to/k3s.kubeconfig \
  --distribution k3s --image aidash:orchestration-local
```

The runner requires an explicit kubeconfig and creates a unique namespace. It
executes the two-node research golden path using local OpenAI/Anthropic protocol
fixtures, kills a worker after an effect, scales server/worker replicas, rolls
every Deployment, stops and resumes workers, and checks stable identities,
original task/run/artifact IDs and three unique external effects. It checks the
real Kubernetes observation API and writes evidence under `.ignore/platform/`.
The Python runner only deletes the namespace it created. `--keep` retains it until Ctrl+C
for browser inspection. The all-six integrated release scenario is separate.

References: [Pod termination](https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#pod-termination),
[probes](https://kubernetes.io/docs/concepts/workloads/pods/probes/),
[Deployment updates](https://kubernetes.io/docs/concepts/workloads/controllers/deployment/),
[kind configuration](https://kind.sigs.k8s.io/docs/user/quick-start/), and
[k3s requirements](https://docs.k3s.io/installation/requirements).
