# GCP Environment dependencies

This chart is the GCP-only companion to `deploy/helm/aidash`. It renders one
PostgreSQL 17 StatefulSet using the existing `deploy/postgres/Dockerfile` image
with pgvector, PGroonga and pg_jsonschema; one NATS JetStream StatefulSet; one
Caddy plus Nginx/Lua edge Deployment; and a minute activity CronJob. It does not
create a GKE cluster, disks through cloud APIs, IAM grants, DNS, or lifecycle
controller actions. Those are Phase 4 integration work.

Install a single shared `storage.createClass=true` class, or reference an
existing class that uses reclaim `Retain` and `WaitForFirstConsumer`. The chart
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

Set `edge.hostname` and `edge.backend` (normally `<aidash-release>-backend`).
The public LoadBalancer exposes only ports 80/443 and uses
`externalTrafficPolicy: Local`, so Caddy receives the original client address.
Caddy replaces client forwarding headers before local Nginx admission. TLS state
is on a kept ReadWriteOnce claim. Nginx starts with admission closed; the trusted
controller opens it only after dependencies, migrations and execution admission
are verified. The internal activity Service exposes port 8089. NetworkPolicy
permits that port only from the activity collector in this namespace; the
controller uses authenticated Kubernetes API port-forward for seal/unseal.
PostgreSQL and NATS accept only same-namespace server/worker and observer traffic.

For shared preview TLS, render `previewTls.createVolume=true` once, with the
existing disk's CSI `volumeHandle`, zone, claim name and claim namespace. The
static PV has `Retain` and is kept across chart uninstall. The trusted controller
must create/bind its explicit `storageClassName: ""`, `volumeName` PVC and rebind
it to the next active pr-N namespace only after the previous edge is stopped and
its attachment is gone. Set `edge.existingTlsClaim` to that claim. Never let Helm
uninstall or a preview switch delete the shared TLS disk. Rebinding belongs to
the gated lifecycle controller; this chart does not guess ownership or detach a
live claim.

`activity.existingSecret` supplies the existing read-only observer `DATABASE_URL`
and `AIDASH_CORE_RUNNER_TOKEN`. The CronJob reads database work, the private edge
activity endpoint, and the Runner's authenticated `/v1/activity`; it gets/patches
only `<release>-environment-activity`. The persisted `snapshot.json` uses the
existing `aidash-infra-activity/1` contract. Failed observations cannot authorize
stop. A gap longer than 120 seconds is busy and starts a full new idle interval;
new transfer receipts and actual work completions also renew it. Receipt IDs only
accumulate, so the snapshot keeps their SHA-256 digest and count rather than the
set, staying far below the ConfigMap size limit; any change renews the interval.
A failed observer leaves an old snapshot that the controller must reject as stale.

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
helm lint infra/gcp/helm/environment --set postgres.existingSecret=db \
  --set edge.hostname=develop.example --set activity.existingSecret=observer
helm template develop infra/gcp/helm/environment --namespace develop \
  --set postgres.existingSecret=db --set edge.hostname=develop.example \
  --set edge.backend=develop-backend --set activity.existingSecret=observer \
  --set activity.runnerEndpoint=http://develop-execution-runner:8949
```
