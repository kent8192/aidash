# GCP Environment dependencies

This chart is the GCP-only companion to `deploy/helm/aidash`. The lifecycle
controller installs it as release `env` in each Environment's `aidash-<id>`
namespace, from the trusted checkout. It renders one PostgreSQL 17 StatefulSet
using the existing `deploy/postgres/Dockerfile` image with pgvector, PGroonga and
pg_jsonschema; one NATS JetStream StatefulSet; one Caddy plus Nginx/Lua edge
Deployment; and a minute activity CronJob. It creates no cluster-scoped objects,
cloud disks, IAM grants or DNS records.

`storage.className` (default `aidash-retain`) names the cluster-wide class with
reclaim `Retain` and `WaitForFirstConsumer`; the controller ensures it. The chart
keeps claims and uses StatefulSet claim-retention policies for both scale-down
and deletion. Server, worker, Runner journal, application objects and Home ledger
claims are separately provisioned; never restore the ledger from a DB backup.
Stopping retains disks; only an explicitly authorized destroy deletes them.
Thirty-second not-ready/unreachable tolerations accelerate eviction; reattachment
is asynchronous and this single-primary profile has a recovery pause.

Supply `postgres.existingSecret` with `POSTGRES_USER`, `POSTGRES_DB` and
`POSTGRES_PASSWORD`. Provision/migrate the application and activation transport
before opening edge admission. Set immutable image digests in deployment values.
Build `edge.Dockerfile` from the trusted ref for Nginx/Lua, and use the same trusted
ref for chart files, the observer, activity collector, Runner, guard and installer.
Their trust boundary is independent of any deployed fork PR source.

Set `edge.hostname` and `edge.backend` (normally `<aidash-release>-backend`, e.g. `app-backend`).
The release name is limited to 31 characters: generated names add up to 21
(`-environment-postgres`), and StatefulSet and CronJob names must leave 11 of
63 characters for their controller-generated suffixes.
The public LoadBalancer exposes only ports 80/443 and uses
`externalTrafficPolicy: Local`, so Caddy receives the original client address.
Caddy replaces client forwarding headers before local Nginx admission. TLS state
is on a kept ReadWriteOnce claim. Nginx starts with admission closed; the trusted
controller opens it only after dependencies, migrations and execution admission
are verified. The internal activity Service exposes port 8089. NetworkPolicy
permits that port only from the activity collector in this namespace; the
controller seals and unseals through
`kubectl exec deploy/env-environment-edge -c admission -- curl -fsS -X POST http://127.0.0.1:8089/admission/{close,open}`,
which is why the trusted edge image includes `curl`.
PostgreSQL and NATS accept only same-namespace server/worker and observer traffic.

Preview TLS uses the existing `aidash-preview-tls` disk as one static `Retain`
PersistentVolume. The controller owns that PV and its `preview-tls` claim, and
rebinds it to the next active pr-N namespace only after the previous edge has no
Pods and no VolumeAttachment for it. Set `edge.existingTlsClaim: preview-tls`
for previews; otherwise the chart creates its own kept TLS claim. This chart never
renders the PV, so Helm uninstall cannot delete the shared TLS disk.

`activity.observerImage`, `activity.collectorImage`, `edge.admissionImage`,
`edge.caddyImage`, `postgres.image` and `nats.image` hold the database credential,
the Runner token, the admission gate, all public traffic or the retained database
and JetStream volumes, so the chart refuses to render them unless they end in
`@sha256:<digest>`. The shipped Caddy and NATS defaults are pinned; the locally
built PostgreSQL, admission, observer and collector images must be supplied.

`activity.existingSecret` supplies the existing read-only observer `DATABASE_URL`
and `AIDASH_CORE_RUNNER_TOKEN`. The CronJob reads database work, the private edge
activity endpoint, and the Runner's authenticated `/v1/activity`; it gets/patches
only `<release>-environment-activity`. The Pod disables automatic token mounting
and projects that ServiceAccount token only into the collector, so the
database-facing observer cannot publish a snapshot. The persisted `snapshot.json` uses the
existing `aidash-infra-activity/1` contract. Failed observations cannot authorize
stop. A gap longer than 120 seconds is busy and starts a full new idle interval;
new transfer receipts and actual work completions also renew it. Receipt IDs only
accumulate, so the snapshot keeps their SHA-256 digest and count rather than the
set, staying far below the ConfigMap size limit; any change renews the interval.
A failed observer writes no database evidence but does not fail the Pod, so the
collector still publishes a fresh busy snapshot; a Pod that never publishes leaves
an old snapshot that the controller must reject as stale.

The Phase 4 idle seal closes admission and gracefully drains **both server and
worker**. Keep the Runner, edge, database and observer running for the fresh
post-drain observation. Active or unavailable sources require restoring both
roles and admission; only an idle verdict permits stopping the Runner and the
rest. Work that starts between the last idle observation and that drain can be
interrupted **even without `force`**. Cluster Node loss and Spot preemption
similarly leave unproven operations uncertain; only the guard's existing
termination evidence permits recovery. The [Phase 4 handoff](../../../../docs/operations/issue-155-phase4.md)
records the producer audit and sequencing gate. Real GKE rescheduling, drain,
static-disk rebinding and IAM verification require later cloud approval.

```sh
# images.yaml sets the digest-pinned postgres.image, edge.admissionImage,
# activity.observerImage and activity.collectorImage.
helm lint infra/gcp/helm/environment -f images.yaml --set postgres.existingSecret=env-postgres \
  --set edge.hostname=develop.example --set activity.existingSecret=env-activity
helm template env infra/gcp/helm/environment --namespace aidash-develop -f images.yaml \
  --set postgres.existingSecret=env-postgres --set edge.hostname=develop.example \
  --set edge.backend=app-backend --set activity.existingSecret=env-activity \
  --set activity.runnerEndpoint=http://app-execution-runner:8949
```
