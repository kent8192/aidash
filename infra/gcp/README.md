# GCP deployment environments

This provisions nonproduction GCP Environments on one shared GKE Standard
cluster using Terraform, a trusted GitHub Actions controller, Helm and
infrastructure-side activity observation. Application and Runner business logic
are unchanged. Nonproduction uses one shared cluster; production uses a separately
bootstrapped project, backend, cluster and runtime identities. See the
[release deployment runbook](../../docs/operations/release-deployment.md) for
the develop-to-main flow, protected environments, random hosts and notification
setup. With every nonproduction Environment stopped, no nonproduction node runs.
Production stays running on normal capacity.

## Environments and lifecycle

| Environment | Source                                                           | Node pool       | Public origin                         |
| ----------- | ---------------------------------------------------------------- | --------------- | ------------------------------------- |
| `develop`   | Configured `develop/x.y.z`                                       | Normal          | `https://<random>.preview.aidash.run` |
| `pr-N`      | Explicitly requested open PR targeting `main` or `develop/x.y.z` | Spot by default | `https://<random>.preview.aidash.run` |
| `test`      | Explicit branch or full SHA, then pinned                         | Spot by default | `https://test.aidash.run`             |

All Environments share the zonal GKE Standard cluster `aidash` in
`us-central1-a` (Dataplane V2, Workload Identity, private Cluster Nodes with Cloud
NAT, Kubernetes 1.34 or later). Each Environment owns:

- namespaces `aidash-<id>` (app), `aidash-<id>-sandbox` (Execution Pods),
  `aidash-<id>-trusted` (guard, which the Runner may exec into) and
  `aidash-<id>-installer` (privileged installer; the Runner has no rights there);
- an Ubuntu/containerd N2 node pool (`n2-standard-4` by default) labelled and
  tainted `aidash.run/environment=<id>`; nothing else schedules there;
- server and worker Google service accounts bound through Workload Identity, a
  Secret Manager runtime secret and, when enabled, GCIP tenants;
- retained PVCs on the `aidash-retain` StorageClass (`pd-balanced`, `Retain`):
  PostgreSQL, NATS JetStream, Runner journal, Home ledger (`memory-recovery`, the
  ledger in its `home/` child) and capability objects (`capability-objects`, the
  store in its `objects/` child). A fresh disk root belongs to root, so the
  application UID creates and restricts those children itself; the migration Job
  mounts the ledger claim root and initializes `home/` before the writers start.

A small untainted E2 system pool runs one node only while any Environment runs.
Boot disks are bounded (system 30 GB `pd-standard`, Environment nodes 50 GB
`pd-balanced`): GKE's 100 GB default exhausts the default 250 GB regional
`SSD_TOTAL_GB` quota, which Environment boot disks share with retained PVCs.
PostgreSQL 17 with pgvector/PGroonga and NATS JetStream are single-replica
StatefulSets from [the Environment chart](helm/environment/README.md); web/API,
workers and the opt-in gVisor execution components come from
[the application chart](../../deploy/helm/aidash/README.md). Execution Pods run
upstream gVisor installed by the privileged installer DaemonSet from the archive
bundled in the trusted `control` image, so a new or recreated Cluster Node needs
only Artifact Registry, not Google's gVisor release bucket.

Only one Environment of each kind may run; other PR requests wait for the preview
slot. Stopped PRs retain their own independent data. `stop` scales the
Environment's workloads and node pool to zero, removes its LoadBalancer and DNS
record and keeps PVCs, Secrets and identities. `resume` restores them.
`destroy` deletes the namespaces, the retained disks bound to their claims, the
node pool, service accounts and runtime secret. A closed/merged PR is destroyed
by reconciliation, regardless of its build result. Reopening it requires a fresh
explicit request.

Each running Environment has its own L4 passthrough LoadBalancer Service for the
edge (ephemeral IP, `externalTrafficPolicy: Local`) and a DNS-only Cloudflare A
record. New staging/PR incarnations receive a 128-bit random hostname beneath
`preview.aidash.run`, stable through updates and stop/resume. The trusted edge
uses a separately provisioned wildcard certificate from Secret Manager; its key
is never mounted in the application or source build. Unknown Host values are
rejected and no per-host ACME certificate is issued. Legacy fixed-host PRs retain
the old preview TLS disk only until a verified update or retirement; new random
hosts never bind that shared claim.

With `release_flow_enabled`, successful protected-develop push CI creates or
resumes staging automatically at that exact current SHA and creates a matching
release PR. Other PR environments still need an explicit authorized request;
opening a PR or completing its CI never creates or wakes a preview. Active
opted-in same-repository PRs follow their current CI-successful SHA. Standalone
test environments remain pinned. Updates wait for active
work and coalesce to the latest authorized source. Explicit normal-capacity
overrides persist across updates.

## Structure and trust boundary

- `bootstrap/`: shared private state/release buckets, Artifact Registry, APIs,
  deployment/publisher identities and GitHub Workload Identity Federation.
- `environments/`: the shared network, Cloud NAT, node service account, GKE
  cluster and system pool, preview TLS disk, broker grants and the DNS-only
  Cloudflare A records. The existing DNS zone is not owned.
- `modules/environment/`: per-Environment node pool, Workload Identity service
  accounts, runtime secret metadata, BYOK grants and GCIP tenants.
- `helm/environment/`: PostgreSQL, NATS, edge (Caddy + Nginx admission) and the
  activity collector chart.
- `control/`: authorized requests, generation fencing, image publication,
  desired-state reconciliation and Kubernetes/Helm lifecycle operations.
- `observer/`: separate Rust program; SeaQuery/SeaORM, read-only transactions,
  no changes to the application schema.
- `tests/`, `check.sh`: provider-mocked Terraform tests, controller regressions,
  real PostgreSQL queries, the edge admission container check and Helm rendering.

WIF trusts numeric repository/owner IDs and exact workflow, execution branch
and GitHub Environment claims (see
[Deploy pipeline boundary](#deploy-pipeline-boundary)). Nonproduction automation executes on the
protected configured `automation_branch`; production cloud jobs execute the
production workflow on `main`. Both check out privileged code and charts from
protected `main`, pinned to the one commit the first job resolved, so every build,
publish and apply job of a run uses the same trusted code. Source images
(`app`, `postgres`, `sandbox`) are built from an exact SHA
in a separate job without cloud credentials, using trusted Dockerfiles. A
`sandbox` source whose `runner/probe.py` does not emit the trusted Runner's
admission probe protocol is refused before it is built.
Privileged images (`observer`, `control` for the Runner/guard/installer and
`edge`) are built only from the trusted checkout, including for fork previews.
The `edge` and `control` images install exact package versions from one Ubuntu
or Debian archive snapshot on a digest-pinned base, so a rebuild reproduces them.
NATS and Caddy use reviewed upstream linux/amd64 digests in `control/images.py`.
A publisher job copies all eight image archives to digest-addressed private
registry entries without executing them; `release.json` records each digest and
the trusted commit that built them. A retained release is deployed only by a
controller running from that commit; production promotes the complete staged
bundle and its original controller revision after main CI, human release merge
and Git tree equality checks; after the default branch advances, a
resume of the same source rebuilds it instead of mixing image and chart
revisions, while running Environments keep their deployed images.
Fork heads require explicit write-collaborator authorization for that exact SHA;
a retained approval covers resuming the same SHA, never a later commit.
Actions/comments require repository write, maintain or admin permission. No
workflow posts PR comments. Retried workflows re-emit a pending build for the
accepted SHA and generation; newer stop/destroy requests still fence it.
Attempt-specific publish tags and replaceable workflow artifacts allow recovery
from partial publication.

The apply job installs `kubectl`, `gke-gcloud-auth-plugin` and pinned Helm, and
fetches cluster credentials through the DNS-based control plane endpoint into a
private kubeconfig. There is no SSH or IAP access; the IP endpoint has no
authorized networks.

Workload Identity is the only path to Aidash permissions. The server's service
account may create Provider Credential secrets and manage those under its
Environment prefix (BYOK) and read its own GCIP tenants. The worker's may only
sign with the Cloud KMS capability key (`roles/cloudkms.signer`). The node service
account has only logging/monitoring writer and Artifact Registry reader roles;
every other Pod, including fork previews, holds no Aidash grants. Fork previews
are isolated by namespace, tainted node pool and NetworkPolicy. Sandbox Pods keep
deny-all networking. The backend accepts ingress only from the edge Pods, and
authentication throttling trusts `X-Real-IP` only from the Pod CIDR: Caddy
replaces client-supplied forwarding headers with its socket client address, and
Nginx overwrites `X-Real-IP`, so separate clients keep separate budgets.

## Initial configuration

Provisioning is intentionally disabled until these account-specific inputs are
configured. The project is never inferred from a developer's `gcloud` default.

1. Choose a billing-enabled **Aidash GCP project** and globally unique bucket
   names. Use an identity allowed to enable APIs, create the bootstrap
   resources and grant their IAM roles. Authenticate Terraform using ADC, for
   example `gcloud auth application-default login`. Select `develop_branch`.
2. Publish the reviewed implementation to protected `main` and the configured
   default `automation_branch` (`develop/0.1.0`) through the normal review process.
   Privileged jobs pin the reviewed controller on `main`, independently of their
   execution branch. Select a deployable source containing Google OAuth,
   trusted-proxy authentication rate limits, the existing runtime contract and
   successful `ci.yml` for its **exact SHA**. The app build explicitly refuses
   sources missing either HTTP integration. Enable production only after its
   isolated account setup and acceptance checks in the release runbook pass.
3. Copy `bootstrap/terraform.example.tfvars` to the ignored
   `bootstrap/terraform.tfvars` and fill the project/bucket IDs. Use Terraform
   1.14.8 (the workflow pin), then review and apply **only the shared foundation**:

   ```sh
   terraform -chdir=infra/gcp/bootstrap init
   terraform -chdir=infra/gcp/bootstrap plan -out=bootstrap.tfplan
   terraform -chdir=infra/gcp/bootstrap apply bootstrap.tfplan
   terraform -chdir=infra/gcp/bootstrap output -json configuration
   ```

4. Migrate the initial local bootstrap state into the new private bucket. Copy
   `bootstrap/backend.tf.example` to the ignored `bootstrap/backend.tf`, then:

   ```sh
   terraform -chdir=infra/gcp/bootstrap init -migrate-state \
     -backend-config=bucket=YOUR_STATE_BUCKET \
     -backend-config=prefix=terraform/bootstrap
   ```

   Confirm the remote state before retiring the local copy. Environment state
   uses the separate `terraform/environments` prefix. Buckets and the registry
   are protected from Terraform destruction and are not PR cleanup targets.

5. Keep the existing Cloudflare `aidash.run` zone/nameservers. Create a token
   scoped to DNS edit for that zone. The three records must be absent or
   deliberately imported before enabling automation; do not overwrite unrelated
   A/AAAA/CNAME records. No Cloud DNS zone or static IP is used; each running Environment publishes its
   ephemeral L4 LoadBalancer IP.
6. Random staging/PR hosts require managed GCIP tenant bindings and the fixed
   Firebase authentication helper. Configure exact OAuth/OIDC/SAML callbacks
   for `https://<project>.firebaseapp.com/__/auth/handler`; the controller manages
   the concrete preview authorized domains. Google OAuth does not accept
   wildcard redirect URIs. Fixed test/production direct OAuth profiles use their
   own exact `/auth/callback` URLs. Configure separate production clients and
   consent-screen test users; never share production runtime secrets with PRs.
   Google login authenticates identity; existing Aidash registration approval,
   member mapping and workspace permissions still control access. Set up the
   initial Deployment Administrator and invited users using the deployed Google integration's
   existing registration/bootstrap procedure.
7. Set the following GitHub repository variables and secrets through Settings
   or `gh variable set` / `gh secret set` with file input. Keep secret values out
   of chat, command arguments, `.tfvars`, plans and Git history.

| Type     | Name                             | Value                                                                                   |
| -------- | -------------------------------- | --------------------------------------------------------------------------------------- |
| Variable | `AIDASH_GCP_CONFIG`              | JSON matching `config.example.json`, using bootstrap outputs and the Cloudflare zone ID |
| Variable | `GCP_WORKLOAD_IDENTITY_PROVIDER` | Bootstrap `workload_identity_provider` output                                           |
| Variable | `GCP_DEPLOY_SERVICE_ACCOUNT`     | Bootstrap `deploy_service_account` output                                               |
| Variable | `GCP_PUBLISH_SERVICE_ACCOUNT`    | Bootstrap `publish_service_account` output                                              |
| Variable | `GCP_ENVIRONMENTS_ENABLED`       | `true` only after setup; otherwise all lifecycle jobs stay disabled                     |
| Secret   | `CLOUDFLARE_DNS_API_TOKEN`       | Zone-scoped Cloudflare token                                                            |
| Secret   | `GCP_DEVELOP_RUNTIME_CONFIG`     | Google/provider configuration JSON for develop                                          |
| Secret   | `GCP_TEST_RUNTIME_CONFIG`        | Google/provider configuration JSON for test                                             |
| Secret   | `GCP_PR_RUNTIME_CONFIG`          | Google/provider configuration JSON for PR staging                                       |
| Secret   | `GCP_DEMO_DATA`                  | People for Demo data (optional, `nonproduction` only)                                   |

The optional `node_count` map in `AIDASH_GCP_CONFIG` (keys `develop`, `test`,
`pr`) sets the Environment node pool size per kind; omitted kinds use one node.

The optional `GCP_DEMO_DATA` Environment secret of `nonproduction` holds the real
people for Demo data (ADR 0080) as one JSON object:
`{"people":[{"email":"a@example.com","tenant":"acme","subject":"alex","role":"administrator"}]}`.
`tenant` names a `key` in `deploy/demo-data/structure.json`, and `role` is
`administrator` or `member`. The apply job passes it to the controller as
`AIDASH_DEMO_DATA`, which stores it as the `app-demo-data` Secret and mounts it
read-only into the migration Job only, never into a server or worker, then
deletes it when the Job ends. Every Environment except production runs
`aidash demo-data` in that Job with `AIDASH_DEMO_DATA_ENABLED=true`; without the
secret it loads only the structure. An Environment built from a fork never
receives the people. Production never loads Demo data: its migration Job has
neither the command nor the setting, and `gcp-production.yml` cannot read the
`nonproduction` secret.

Without GCIP, each runtime JSON contains string values for `AIDASH_OIDC_CLIENT_ID` and
`AIDASH_OIDC_CLIENT_SECRET`, plus required provider credentials named
`AIDASH_SECRET_*`. Optional Google session lifetime settings are
`AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS` and `AIDASH_OIDC_SESSION_IDLE_SECONDS`.
When GCIP is enabled, omit all `AIDASH_OIDC_*` keys. The controller adds the
public `dashboard.gcip` fragment to the same JSON; provider credentials remain
flat `AIDASH_SECRET_*` strings. The controller passes only the GCIP fragment to
the app chart's `gcip.settings`, which mounts it read-only and selects it through
`AIDASH_GCIP_SETTINGS`. Reinhardt composes this source with the normal server
settings, applies typed defaults and validates the sole issuer. Legacy OIDC
settings are not exported in this mode. Removing Tenant Bindings updates the
settings before the Environment is reopened.

The controller generates private database/API/Runner/observer keys once into the
namespace's `aidash-identity` Secret and never regenerates them (a key added later
is generated once for existing Environments); callers cannot
override these or the node identity through runtime JSON. The activity observer
connects as its own `aidash_observer` PostgreSQL role, never the application's
administrative `aidash` login. After every migration (and on every settings
reload) the controller creates or updates that role through the PostgreSQL
container's local socket, with SQL and password on stdin: `LOGIN` only, `SELECT`
on exactly the tables the observer queries, and `pg_read_all_stats`, without
which `pg_stat_activity` would hide other sessions' in-flight work. Terraform creates secret
metadata only. The controller uploads an initial secret version via stdin, reads
the allowlisted keys (`AIDASH_OIDC_*`, `AIDASH_SECRET_*`,
`AIDASH_PROVIDER_FINGERPRINT_KEY`) and materializes the `app-runtime` Kubernetes
Secret consumed by server and worker. No Pod identity can read Secret Manager
runtime secrets, and secret values do not enter Terraform state.

## Operation

Run **GCP environments** using workflow ref `main`; `source_ref` separately
selects the deployed source. A manual create/resume first checks CI. Examples:

```sh
gh workflow run gcp-environments.yml --ref main \
  -f action=create -f environment=develop
gh workflow run gcp-environments.yml --ref main \
  -f action=create -f environment=test -f source_ref=develop/0.1.0
gh workflow run gcp-environments.yml --ref main \
  -f action=resume -f environment=test
gh workflow run gcp-environments.yml --ref main \
  -f action=stop -f environment=test
```

Use `action=destroy` to delete owned data. Use `vm_mode=normal` on an explicit
test/PR resume to choose normal capacity; blank/default preserves the current
mode. A test resume without a source keeps its pinned SHA. Supply a new source
explicitly to deploy onto retained test data; migration compatibility still
applies. Retargeting shared develop requires destroy/create.

On the PR, an authorized collaborator can write exactly one of:

```text
/preview up
/preview up normal
/preview stop
/preview destroy
/preview up spot FULL_40_CHARACTER_REVIEWED_FORK_HEAD_SHA
```

For Actions use `environment=pr`, `pr_number=N`, and `approved_sha` for a new fork
head. The illustrative fork SHA above must be replaced with the actual hash.
No environment is created by opening a PR alone. Command acceptance and cloud
authentication require a complete preview command beginning with `/preview `
and current repository write/maintain/admin permission. A separate intake job
has no cloud or OIDC permissions; ordinary issue/PR comments and unrelated
authors are excluded before it starts, and malformed/unauthorized commands
cannot start the prepare/apply jobs.

Command acceptance and cloud
reconciliation share a durable lock, so an older apply cannot scale up a node pool
after a newer stop/destroy request has been accepted. Preparation waits up to 450
seconds for the lock; if that expires, the command has not been accepted and the
Actions run fails. Retry after the lock owner finishes. Locks are never stolen.
Preparation has at most eight minutes of controller work, and reconciliation has at
most forty minutes across all environments. These budgets shrink by the setup
time already spent in the current Actions job, preserving time before its
10/55-minute hard timeout. Expiration exits retry loops, interrupts and reaps CLI
process groups, bounds failure cleanup to three minutes, and reserves a separate
90 seconds for deleting only the owned lock generation. A started deployment
that runs out of time stays failed/gated until an explicit retry; untouched
environments can be reconciled by a later scheduled run. Forced runner loss or
unavailable cloud credentials still require the lock recovery procedure below.
Builds run outside the lock and attach only to their accepted generation. A later
scheduled reconciliation can process accepted intent or a completed build if its
immediate apply job finds the lock busy. Inspect Actions
logs and `lifecycle/state.json` in the private state bucket for ready/pending/
draining/`waiting_for_active_work`/`waiting_for_preview_tls`/
`waiting_for_gcip_refresh`/interrupted/failed state, source and digests. A build artifact alone is not proof
of a successful deployment.

A deployment brings the Environment node pool (and the system pool) up, ensures
the `aidash-retain` StorageClass, namespaces, Secrets and retained PVCs, installs
the `env` chart (PostgreSQL, NATS, edge, activity collector), runs a one-shot Job
for migrations, Home ledger initialization and activation provisioning, then
installs the `app` chart. Readiness requires the server Deployment's
`aidash.run/source-sha` annotation to equal the authorized SHA, complete rollouts,
a ready Runner and the public HTTPS node identity. The controller verifies the rollout/source, publishes the edge LoadBalancer IP
in DNS, opens admission, then verifies public HTTPS and node identity before
marking ready and queuing notifications. A timeout leaves the
generation failed for manual inspection, without replaying unknown work.

## Idle stop, updates and failure recovery

The edge counts admitted sign-in/submission/save/execution requests, including
in-flight writes not yet visible in PostgreSQL, and the dashboard's pointer or
keyboard interaction heartbeat (`POST /auth/activity`, at most every 15 seconds).
Polling, session checks, token refresh, logout, health checks, reading,
scrolling and unsent drafts do not renew the deadline. The activity CronJob
samples durable work every minute into its ConfigMap. The read-only observer
covers Runs, leases, capability operations, verification, generation,
transactions and active transfers; the Runner's `/v1/activity` covers pending and
unproven operations. Completed work and explicit resume renew the idle deadline.
Transfer receipts have no completion timestamp: a newly observed completion
starts a full idle hour at observation time. Pure human-input/approval waits may
idle; approval expiration remains wall-clock based.

Before stop, update, idle stop, broker change or BYOK destroy, the controller
seals the Environment as described in the
[Phase 4 handoff](../../docs/operations/issue-155-phase4.md): close edge
admission, scale server and worker to zero and wait for their Pods to exit, then
run the activity Job and require a fresh observation. Busy, failed, stale or
unavailable observations restore server, worker and admission and defer. Only an
idle verdict permits stopping the Runner, edge and StatefulSets. Work started
between the last idle observation and the drain can be interrupted **even
without `force`**. Explicit `force=true` skips the observation. Neither drain
completion nor Kubernetes eviction is termination proof; uncertain operations are
never replayed by infrastructure.

Admission is also recorded durably in the controller-owned ConfigMap
`env-environment-admission` (`state: open|closed`), which every edge reads at
start: closing records `closed` first, opening records `open` last, and deploys
and stops record `closed`. A rescheduled or restarted edge (Spot preemption, node
loss, container restart) therefore resumes serving without a controller call, and
any partial failure leaves it closed. An Environment without live Pods is already
sealed.

The nonproduction stop threshold is 60 minutes. Production has no idle stop,
Spot capacity or automatic destroy path. Reconciliation is scheduled every five minutes
and GitHub scheduling/startup can be delayed; **this is not an exact shutdown-time
SLA**. PR-close cleanup has the same scheduling delay. If Actions is disabled or
observation/IAM fails, an Environment can keep accruing charges; inspect failed
runs. No idle timer resumes a stopped Environment.

- **Zone loss in production:** production's retained disks are regional and its
  disk-holding workloads are StatefulSets under Stateful HA, so they move to the
  standby pools in `us-central1-b`, which scale up from zero (ADR 0067). Only
  the cluster autoscaler grows them, and it halts when more than three nodes
  and more than 45% are NotReady, so production is limited to two Environment
  nodes beside its system node; if it halts anyway, resize the standby pools by
  hand as in the release deployment guide. The
  `aidash-retain` StorageClass in production's own cluster is created regional,
  and the controller refuses a production cluster whose class is zonal, because
  class parameters cannot change. Nonproduction stays in one zone.
- **Node loss and Spot preemption:** GKE recreates the node and Kubernetes
  reschedules the Pods; retained disks reattach to the replacement. There is no
  power observation or automatic normal-capacity fallback. In-flight operations
  surface as uncertain through the existing Runner recovery and are not replayed.
  If Spot capacity is unavailable, the pool stays unscheduled: stop, or resume
  with `vm_mode=normal`.
- **Deployment/migration/readiness failure:** data is retained and DNS withdrawn;
  no destructive rollback or database downgrade is attempted. Repair the cause
  and use explicit `resume` (with `force=true` if admission observation is broken).
  A failed deployment generation is not retried by cron. HTTPS readiness checks
  the Environment's node identity, not just an HTTP 200 from an old preview IP.
- **Runtime credential rotation:** add a new version to that Environment's
  Secret Manager secret using secure file/stdin input, then explicitly resume
  with `force=true` to rematerialize `app-runtime`. Changing a GitHub seed secret
  affects newly created Environment secrets, not already populated secrets.
- **Abandoned lifecycle lock:** read `lifecycle/apply.lock`, inspect its GitHub run
  and confirm that no intake or reconciliation still runs before removing that exact GCS generation.
  Locks are never stolen on a timeout. Preserve `lifecycle/state.json`, including
  tombstones; deleting it loses stale-run fencing. Terraform's own backend lock
  is separate and also requires proving the owner has stopped before recovery.
- **Partial Terraform apply:** inspect the plan and state before retrying. The
  controller rejects plans deleting the cluster, the preview TLS disk or KMS keys,
  deleting a node pool outside an explicit retirement, or scaling a node pool up
  from zero for an Environment that is not starting. Do not reset state or
  delete retained PVs to get a clean plan.

Inspect workloads with `kubectl -n aidash-<id>` using credentials from
`gcloud container clusters get-credentials aidash --location us-central1-a --dns-endpoint`.
Avoid publishing configuration, database URLs or logs containing private
application data.

## Validation and remaining deployment gates

Run `bash infra/gcp/check.sh` with Terraform, Rust, Helm and Docker installed. It
creates only disposable local test containers and uses mocked cloud providers.
Also run Ruff, actionlint and `git diff --check` for changed Python/workflows.
Main CI requires the reusable **GCP infrastructure checks** job.

The local checks do not establish real GKE behavior: Cluster Node loss, Spot
preemption, persistent disk reattachment, preview PV rebinding, Workload Identity
and IAM, Google login, SSE, DNS/TLS or a measured capacity limit. Before inviting
users, exercise create, a real Shell/Python/file operation, long-running-work
drain, one idle hour, stop/resume, PR replacement/close, Spot recovery and
deletion; see the [Phase 4 handoff](../../docs/operations/issue-155-phase4.md).
The new production workflow does not establish production backup/availability
acceptance. Agree and verify the separate backup, restore, IAM, monitoring and
recovery gates before enabling it; retained PVCs are not backups.

## Cost and external references

The combined budget target is JPY 10,000/month, including node pool runtime, the
system node while any Environment runs, retained disks, LoadBalancer forwarding
rules and IPv4, Cloud NAT, registry/state storage, traffic, secrets, logs,
applicable Actions charges and tax; LLM/API costs are separate. The GKE cluster
management fee is covered by the free tier for one zonal cluster per billing
account; verify current terms. This target is not a billing cap. Retained images
are not automatically expired because stopped Environments must remain
resumable. Periodically remove only versions no longer referenced by
desired/applied lifecycle state. Scheduled workflows also consume Actions
capacity even when every Environment is stopped.

Implementation references: [GKE Workload Identity](https://cloud.google.com/kubernetes-engine/docs/how-to/workload-identity),
[GKE Spot VMs](https://cloud.google.com/kubernetes-engine/docs/concepts/spot-vms),
[gVisor containerd setup](https://gvisor.dev/docs/user_guide/containerd/quick_start/),
[GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows),
[Google WIF](https://docs.cloud.google.com/iam/docs/workload-identity-federation-with-deployment-pipelines),
[Cloudflare DNS Terraform](https://developers.cloudflare.com/api/terraform/resources/dns/subresources/records/).

## Provider Credentials, broker and BYOK

Provider Credential Key Material uses a separate existing, billing-enabled BYOK
project. Set `byok_project_id` explicitly in bootstrap and deployment configuration
to enable BYOK provisioning; it must differ from `project_id` and contain only
Provider Credential secrets. Omit it or leave it empty for existing deployments
without the Store: no BYOK API, project lookup, role, audit configuration or runtime
grant is provisioned. Existing shared-project lifecycle operations remain available.
Before clearing or replacing an enabled `byok_project_id`, retire all managed
environments using the applied project. The controller refuses a mismatched
project before observing Environments or applying Terraform while environment inventory
remains; cleanup must verify each old prefix is empty before removing its runtime
identities and disks. Enabling BYOK on a legacy deployment remains supported.

Bootstrap enables Secret Manager and its DATA_READ/DATA_WRITE audit logs there.
Human-run bootstrap defines `aidashByokCreate`, `aidashByokManage` and
`aidashByokBrokerRead` once per deployment. Environment automation binds each Environment's
server Workload Identity service account to the create-only role without a condition and the seven-permission
manage role with their environment's Secret name prefix using the numeric project
number. Runtime identities have no BYOK payload access or IAM-setting permission.
Pods have no Secret Manager runtime configuration access; the controller
materializes runtime configuration as a Kubernetes Secret.

BYOK-enabled Environments pass the non-secret Store descriptor to the app chart's
`providerCredentials.settings`; the chart mounts it read-only into migrations and
the app, and Reinhardt loads it via `AIDASH_PROVIDER_CREDENTIAL_SETTINGS`. The
runtime configuration secret must also include a stable independent
`AIDASH_PROVIDER_FINGERPRINT_KEY` of at least 32 bytes. Startup refuses a missing
or short key; deployments never regenerate it. Keep that key unchanged across
upgrades and restarts. It stays in the `app-runtime` Secret, never in the public
descriptor. Disabled BYOK renders no Store and needs no fingerprint key. The
managed descriptor uses `fingerprint_key = {env = "AIDASH_PROVIDER_FINGERPRINT_KEY"}`
and `store.kind = "secret_manager"`; the fingerprint reference stays outside the
Registry-accessible `AIDASH_SECRET_*` namespace. Store-only environments use a
null broker descriptor; enabled brokers add their non-secret endpoint, issuer,
audience and signing-key version to the same settings source.

The deploy identity's BYOK role contains only `resourcemanager.projects.get`,
`getIamPolicy` and `setIamPolicy`. Its binding uses exactly
`api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly(['projects/<byok>/roles/aidashByokCreate', 'projects/<byok>/roles/aidashByokManage'])`.
It has no BYOK `iam.roles.*` permission and cannot directly grant BrokerRead or
change its own BYOK binding. Google documents this restriction for project
`setIamPolicy` in [Set limits on granting roles](https://docs.cloud.google.com/iam/docs/setting-limits-on-granting-roles)
and lists Projects/Resource Manager in the [IAM API attribute reference](https://docs.cloud.google.com/iam/docs/conditions-attribute-reference#iam_api_attributes).

Environment retirement uses two additional fixed roles bound only by human-run
bootstrap. `aidashByokRetire` contains `secretmanager.secrets.delete` and has the
condition `resource.name.startsWith('projects/<byok-number>/secrets/aidash-')`.
`aidashByokRetireInventory` contains only `secretmanager.secrets.list`, with an
unconditioned project binding because [listing is authorized on the parent
project](https://docs.cloud.google.com/secret-manager/docs/reference/rest/v1/projects.secrets.list).
Neither role contains payload, version-read or IAM permissions, and neither is
in deployment's Create/Manage role-grant allowlist. Residual: deploy can see
BYOK secret names through project-level listing. The application still uses its
PostgreSQL inventory and never lists Secret Manager secrets.

Before deleting an environment's retained disks, identities or runtime IAM,
retirement collects every inventory page and deletes only names starting with
`aidash-<environment_id>-cred-`, then confirms that prefix is empty. Already
deleted secrets are skipped on retry. Existing running app writers are sealed
first; stopped Environments are cleaned without a wake. Failure keeps the
retained disks and identities for retry. This does not add effective
delete power: deployment already controls bindings of Manage, which includes
delete. Deployment remains the trust root under #151; bootstrap owns these
fixed retirement grants and deployment cannot re-grant them or any read role.

`aidashByokBrokerRead` contains `secretmanager.versions.access`, `versions.get`
and `secrets.get`. Human-run bootstrap alone binds it to the broker identities
listed in `byok_broker_environments`, with each environment's Secret name prefix.
Deployment and runtime identities receive no BYOK payload read grant. Outputs
`byok_project_id` and `secret_prefix` supply the broker's secret namespace.
The same human-run bootstrap owns permanent signing keys and exports
`broker_signing_keys`. It grants the deploy service account
`roles/cloudkms.publicKeyViewer` on each signing CryptoKey so deployment plans
can fetch verification PEMs; this grant does not authorize signing. Cloud KMS
Admin does not provide the required
[`cloudkms.cryptoKeyVersions.viewPublicKey`](https://docs.cloud.google.com/kms/docs/reference/rest/v1/projects.locations.keyRings.cryptoKeys.cryptoKeyVersions/getPublicKey)
permission. Environment automation consumes their IDs and removes
only service/signing bindings on disable or retirement. Keep the bootstrap
environment set to preserve immutable KMS names. See the
[broker deployment guide](../../enterprise/crates/aidash-broker/README.md) for the input
contract and manual import steps for any pre-merge draft deployment.

### Deploy pipeline boundary

The deploy pipeline (GitHub Actions through Workload Identity Federation, acting as
the `aidash-deploy` service account) remains part of the v1 trust root for Key
Material (ADR 0005, decided 2026-10-09, #151). Two escalation paths exist; the first
is closed in code, the second is hardened and detected but accepted.

**Path 1, impersonation, is blocked.** The deploy service account holds
`roles/iam.serviceAccountAdmin` in the shared project, which includes
`iam.serviceAccounts.setIamPolicy`. Human-run bootstrap (`bootstrap/deploy_boundary.tf`)
tags every Credential Broker service account with `<project>/aidash-credential-broker=broker`
and attaches the project deny policy `aidash-deploy-broker-boundary`. For the deploy
service account, on resources carrying that tag, it denies:

| Permission (deny-policy name)                           | Why                                |
| ------------------------------------------------------- | ---------------------------------- |
| `iam.googleapis.com/serviceAccounts.setIamPolicy`       | grant itself token creation        |
| `iam.googleapis.com/serviceAccounts.getAccessToken`     | mint a broker access token         |
| `iam.googleapis.com/serviceAccounts.signBlob`           | sign as the broker                 |
| `iam.googleapis.com/serviceAccounts.signJwt`            | sign as the broker                 |
| `iam.googleapis.com/serviceAccounts.implicitDelegation` | use the broker in a delegate chain |
| `iam.googleapis.com/serviceAccountKeys.create`          | create a broker key                |
| `iam.googleapis.com/serviceAccounts.delete`             | delete, then restore, a broker     |
| `iam.googleapis.com/serviceAccounts.undelete`           | restore a deleted broker account   |
| `iam.googleapis.com/serviceAccounts.disable`            | disable a broker account           |
| `iam.googleapis.com/serviceAccounts.getOpenIdToken`     | mint an OpenID token as the broker |

Every other service account keeps working: the deploy identity still administers
the runtime and node accounts. `iam.serviceAccounts.actAs` stays allowed because
Cloud Run needs it to run the broker; bootstrap grants it
(`roles/iam.serviceAccountUser` on each broker account), because the environments
root can no longer bind anything on those accounts. The deploy identity holds no
tag, deny-policy, monitoring or logging role, so it cannot untag a broker account,
edit the policy or change alert policies. Turning off the Secret Manager audit
configuration in the BYOK project is a separate route, covered by the BYOK project
IAM alert below. `tests/test_deploy_boundary.py` classifies every
role bound to the deploy identity and fails when a role carries one of these
permissions without being covered by the deny policy, or when a new role has not
been classified.

**Path 2, broker redeploy, is the accepted residual risk.** The deploy identity can
`actAs` the broker account and holds `roles/run.admin`, so it can ship a modified
broker image, or any other Cloud Run service or job, Compute instance or GKE node
pool that runs as that account, and read Key Material as the broker. It is hardened and detected,
not prevented:

- The broker image digest is reviewed code (`environments/broker-image.json`).
  Terraform derives `us-central1-docker.pkg.dev/<project>/aidash/broker@<digest>`
  from it and refuses an enabled broker without an approved digest or with a
  different image. A broker change is therefore a digest change in a pull
  request to `main`. The deployment configuration (`AIDASH_GCP_CONFIG`) cannot
  override the digest: the controller refuses a `credential_brokers.<id>.image`
  key.
- Credentialed jobs run in protected GitHub Environments named by the WIF
  condition; production deployments need the reviewed `production` (below).
- Bootstrap-owned log alerts, sent to `security_alert_emails`, fire on:
  - a new revision of a broker service, and on a Cloud Run service or job, Compute
    instance (`insert`, `bulkInsert`, `setServiceAccount`, templates) or GKE cluster
    or node pool create/update whose request names a broker account;
  - any IAM policy change of a broker service or broker service account;
  - `AccessSecretVersion` of BYOK secrets by any principal other than the broker
    accounts. That is the only DATA_READ method of Secret Manager; `GetSecret`,
    `GetSecretVersion` and `ListSecrets` are ADMIN_READ, which the BYOK audit
    configuration does not enable, so the server's and deploy's routine metadata
    reads produce no entries to alert on;
  - any audit-config change to the BYOK project, and any project IAM change there
    by a principal other than the deploy identity (Admin Activity log, which cannot
    be turned off). The deploy identity's `setIamPolicy` there is limited by
    `modifiedGrantsByRole.hasOnly(...)`; whether that attribute also constrains
    `auditConfigs` is unverified (a #152 check), so a deploy change that drops
    `DATA_READ` and carries an audit-config delta alerts, but deploy's binding-only
    runtime-role changes stay quiet.

Alerts cannot tell a malicious workload that runs as a broker account from the
real broker: its reads come from an excluded principal, so only the revision and
deployment alerts detect it. They only cover the services listed above; any other
service that can run as a broker account (for example Cloud Functions or Batch) is
not alerted on. The deny policy covers deletion and disabling of broker accounts.

**Deferred.** A separately deployed credential enclave that the deploy identity
cannot reach is deferred, for Enterprise or for growth in key count. Building the
broker in the trusted pipeline, so its digest lands in `release.json` like the
other images, is also deferred; today the broker image is built manually.

#### Protected deploy Environments

Each WIF-trusted workflow runs its credentialed jobs in exactly one GitHub
Environment, and the bootstrap provider condition names that Environment next to
the repository IDs, the exact workflow ref and the execution branch:

| Workflow                       | Credentialed jobs             | Environment                | Execution branch          | Protection                                          |
| ------------------------------ | ----------------------------- | -------------------------- | ------------------------- | --------------------------------------------------- |
| `gcp-environments.yml`         | `prepare`, `publish`, `apply` | `nonproduction`            | exact `automation_branch` | exact-branch policy                                 |
| `gcp-production.yml`           | `prepare`, `apply`            | `production`               | `main`                    | `main` only; **required reviewers**, no self-review |
| `deployment-notifications.yml` | `nonproduction`, `production` | `deployment-notifications` | exact `automation_branch` | exact-branch policy; separate reader identity       |
| `deployment-certificate.yml`   | `renew`                       | `certificate-renewal`      | exact `automation_branch` | exact-branch policy; separate renewal identity      |

A nonproduction bootstrap trusts only the `nonproduction` deploy assertion and a
production bootstrap only the `production` one, so neither tier's workflow can
obtain the other tier's identity. GitHub only emits the `environment` claim for
jobs that declare an Environment, so a job on an unprotected ref or without its
Environment cannot obtain credentials. The `intake` and `build` jobs and
`gcp-check.yml` hold no cloud credentials and need no Environment.
`infra/delivery/github` creates these Environments and their branch policies.

What the production reviewer gate covers: every production deployment, which only
promotes the staged, already validated image bundle. Nonproduction builds and
deploys CI-approved commits without an Environment approval; all of its runs use
privileged code from the protected `main` checkout, so the broker digest and IAM
changes are gated by review of the pull request to `main`.

#### Human steps

1. **Merge this change to `main` first.** The workflow change is harmless before
   WIF requires it; applying the new attribute condition first would lock out the
   current workflow.
2. **Create the protected GitHub Environments** with `infra/delivery/github` (see
   `docs/operations/release-deployment.md`); it disables administrator bypass on
   each.
3. **Apply bootstrap** with the new inputs in `bootstrap/terraform.tfvars`:
   `security_alert_emails` (required once `byok_broker_environments` is set). Whoever applies it needs deny-policy
   (`roles/iam.denyAdmin`), tag administration (`roles/resourcemanager.tagAdmin`
   plus tag user on the service accounts), alert-policy and notification-channel
   administration and Service Usage on both projects. Review the plan: it adds the
   tag key and value, one tag binding per broker account, the deny policy, three
   alert policies, email channels, the `actAs` grants and the new WIF condition.
4. **Approve and record a broker image digest** (below) before enabling a broker.
5. _Only if a broker is already live from an earlier draft:_ let the environments
   root from this commit apply before bootstrap applies the deny policy. It removes
   the old `google_service_account_iam_member.deploy` binding on the broker account
   (bootstrap owns that grant now); after the deny exists, that removal would call
   the denied `setIamPolicy` and fail. No broker has been applied at the time of
   writing.

#### Updating the broker image digest

1. Build from the reviewed commit: `docker buildx build --platform linux/amd64
--provenance=false --sbom=false -f enterprise/crates/aidash-broker/Dockerfile -t
us-central1-docker.pkg.dev/<project>/aidash/broker:<sha> --push .`
2. Read the manifest digest: `docker buildx imagetools inspect
us-central1-docker.pkg.dev/<project>/aidash/broker:<sha>`.
3. Put it in `infra/gcp/environments/broker-image.json` (`{"digest": "sha256:..."}`)
   in a pull request; reviewers verify it against the reviewed commit.
4. After merge, the next apply deploys that digest. Never set `image` in
   `AIDASH_GCP_CONFIG`.

The Dockerfile pins its base images by digest, but the build is not bit-for-bit
reproducible: `apt-get update` and Cargo cache mounts are not pinned. A reviewer can
rebuild and compare behavior, not the digest. The registry also accepts pushes from
the deploy identity; only the digest in `main` is ever deployed, and a revision
deployed outside Terraform triggers the revision alert.

#### Evidence still required from #152 (sandbox)

Nothing in this change was applied to a cloud project. Local tests prove the
Terraform shape against a mocked provider only. The Issue's acceptance criteria
need these results from a disposable sandbox project with a copy of the bootstrap
(commands are examples; run as the deploy service account unless stated):

1. **Deny support.** Google lists all ten permissions as supported in
   [deny policies](https://docs.cloud.google.com/iam/docs/deny-permissions-support),
   but confirm in the sandbox: `gcloud iam policies get
aidash-deploy-broker-boundary --attachment-point=cloudresourcemanager.googleapis.com/projects/<project> --kind=denypolicies`,
   then for the broker account `B` run
   `gcloud iam service-accounts add-iam-policy-binding B --member=... --role=roles/iam.serviceAccountTokenCreator`,
   `gcloud auth print-access-token --impersonate-service-account=B`,
   `gcloud iam service-accounts sign-blob`, `sign-jwt`, `keys create`, and an
   impersonation chain that lists `B` as an intermediate delegate
   (`implicitDelegation`). Each must fail with a deny-policy `PERMISSION_DENIED`.
   Repeat for `serviceAccounts.delete`, `undelete`, `disable` and
   `getOpenIdToken`. Record any permission that is not denied.
2. **Tag targeting.** Confirm service accounts accept the tag binding
   (`gcloud resource-manager tags bindings list --location=global
--parent=//iam.googleapis.com/projects/<project>/serviceAccounts/<unique-id>`) and that the
   deny applies only to tagged accounts: the same `setIamPolicy` succeeds on an
   untagged control account (Workload Identity binding for a runtime account must
   keep working), fails on `B`, and succeeds on `B` again after a human removes the
   tag binding. If service accounts cannot carry tags or `resource.matchTag` does
   not match them, the fallbacks are a project-wide deny without a condition of
   the five impersonation and key permissions (the deploy identity needs none of
   them anywhere), plus a separate decision for `setIamPolicy`, which it does need
   on runtime accounts. Also prove the deploy identity cannot detach the tag
   binding (`gcloud resource-manager tags bindings delete`), and that deleting and
   undeleting a broker account does not restore access: the tag binding, the deny
   and the `actAs` grant must all still apply afterwards (or deletion stays
   denied).
   Also check that `modifiedGrantsByRole.hasOnly(...)` on the BYOK project
   rejects an `auditConfigs`-only `setIamPolicy` by the deploy identity; if it
   passes, the Key Material alert can be blinded and the BYOK project IAM alert
   is the only defence.
3. **Unprotected refs.** Run the workflow from a feature branch, and from the
   execution branch with a job lacking `environment:`; neither may obtain
   credentials. A nonproduction job in `nonproduction` on `automation_branch` must,
   a nonproduction job must not obtain the production identity, and a production
   `apply` must wait for the `production` reviewer. Log the OIDC claims
   (`environment`, `ref`, `workflow_ref`) of each.
4. **Alerts fire.** Deploy a new broker revision; set a broker service IAM policy
   and a broker account policy as a human; call `AccessSecretVersion` on a BYOK
   secret as a non-broker identity (a denied attempt counts). Confirm each
   reaches `security_alert_emails`, and that the broker's own reads do not. Verify
   the audit method names, the broker account's `resourceName` shape (email versus
   unique ID), that the structured and substring `protoPayload.request` matches fire for
   a Cloud Run service or job, a Compute instance and a GKE node pool created
   with a broker service account, that the `data_access` log exists, and that
   removing the BYOK audit configuration raises the BYOK project IAM alert.

## GCIP sign-in

Enable `gcip_enabled` in bootstrap and supply `environment_domains` matching the environment stacks' fixed domains. Bootstrap enables Identity Platform multi-tenancy and initializes the Firebase helper and fixed test or production domain. The lifecycle controller manages concrete random staging/PR authorized domains. Export the sensitive `gcip_web_api_key` output into the public SDK configuration field; it is not an Admin API credential.

Fixed test/production environments can serve Firebase's sign-in helper from
their public origin. Random staging/PR hosts retain the fixed Firebase helper
and manage exact authorized domains; they never use wildcard OAuth callbacks. The controller sets `auth_helper = "firebase"` for random hosts and `auth_helper = "public_origin"` for fixed test/production hosts in the same Terraform apply that deploys a release. The module switches the SAML ACS URL and outputs that setting together with the deployment. The controller copies the output into the runtime secret and, for `public_origin`, sets the `env` chart's `edge.firebaseProject`. An older running release keeps its existing helper until an authorized deployment. With `public_origin`, the edge's Caddy transparently proxies `/__/auth/*` and `/__/firebase/init.json` to `https://<project>.firebaseapp.com`, ahead of Nginx admission. It strips `Cookie` and `Authorization` from upstream requests and `Set-Cookie` from upstream responses. The helper grants no authority: `POST /auth/gcip/exchange` stays behind admission, so a draining Environment still refuses sign-in. Google and OIDC redirect URIs live at the identity providers, outside Terraform. Register these exact URIs with every Google OAuth client and OIDC provider used by `gcip_tenants`, and as the ACS URL at every SAML IdP:

`https://<project>.firebaseapp.com/__/auth/handler` for random staging/PR,
`https://test.aidash.run/__/auth/handler` for test, and
`https://<production-hostname>/__/auth/handler` for the separate production project.
Register them before rolling out a release that sets `auth_helper`. Retain the fixed Firebase callback while random staging/PR hosts use it. Email action links stay on `<project>.firebaseapp.com`, because Identity Platform has one action URL per project.

Supply `gcip_tenants` to the environment controller configuration. For example, `{"acme":{"tenant":"acme","password_sign_up":true,"google_client_id":"CLIENT_ID","sign_in_domains":["acme.example"]}}` creates a fresh pool per environment incarnation. A pool without `tenant` is the Environment's Shared Pool, for example `{"shared":{"password_sign_up":true,"google_client_id":"CLIENT_ID"}}`: Google and email/password sign-in for everyone no Sign-in Domain routes elsewhere. At most one pool may omit `tenant`, and it has no SSO providers or Sign-in Domains; Tenant Bindings are optional. OIDC and SAML provider maps use `oidc.*` and `saml.*` IDs; those SSO pools must disable password signup. `sign_in_domains` lists lowercase ASCII email domains (IDN as `xn--` punycode, exact match, no wildcards) that route `POST /auth/login` email discovery to that pool; a domain may appear in only one pool. It is routing only and grants no authority: membership is still decided by the pool and Tenant Binding, and removing a domain only stops routing. Staff verify domain ownership out of band, and review must reject public mail domains (for example `gmail.com`). OAuth client secrets are separate `gcip_idp_secrets` Terraform inputs, supplied by `AIDASH_GCIP_IDP_SECRETS` in the controller (the lifecycle workflow reads the repository secret `GCP_GCIP_IDP_SECRETS` as this JSON map). They are confined to private inputs and protected Terraform state. Each module outputs its GCIP Tenant IDs, runtime service-account email, Shared Pool, Tenant Bindings, providers and password-signup list. The controller merges these settings into the environment runtime secret, preserving its other settings and refusing coexistence with OIDC.

The controller records digests of both desired shared inputs (Tenant configuration, IdP secrets and web API key) and actual public GCIP outputs, including generated Tenant IDs. Before applying changed shared inputs or a plan that changes GCIP tenant resources, in every affected running Environment, including unpublished ones, it closes edge admission and scales server, worker and Runner to zero under the existing lifecycle lock and state. It never calls an HTTP endpoint on a paused or absent application and requires confirmed quiescence before Terraform can apply.

After every apply, including unrelated lifecycle work, the controller compares actual GCIP outputs and refreshes IAM and runtime secrets for changed retained environments. Published Environments upgrade the `app` release of their existing authorized source with the new policy before reopening admission. Stopped environments receive the new secret without being started. Unpublished Environments remain fenced until an authorized deployment succeeds. Failed refreshes retain a pending marker for scheduled retry; successful environments are not restarted again on that retry. A fenced Environment cannot pass through ordinary seal/unseal rollback with its old policy: non-forced stop or redeployment waits for policy recovery, while the existing explicit force operation can stop the Environment or repair the deployment. No IdP secret or unhashed private input is copied into lifecycle state.

The Google 7.46.1 provider has no tenant IAM resource. The approved controller adapter therefore calls [tenant getIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/getIamPolicy) and [tenant setIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/setIamPolicy). It adds `roles/identityplatform.viewer` only on that environment's tenant resources for that environment's runtime principal, preserving unrelated/conditional bindings and policy metadata, using etag concurrency and bounded conflict retries. Retirement removes only its managed viewer member before Terraform destroys the environment. Runtime principals receive no project-wide Firebase user access.

### Tenant MFA

Each pool declares its MFA Requirement as `mfa.state` in `gcip_tenants`: `disabled` (the default), `enabled` (users with an enrolled second factor are challenged) or `mandatory` (every user must hold a second factor). Only password and Google pools may use `enabled` or `mandatory`: GCIP applies tenant MFA to every first-factor provider except phone, anonymous and Apple Game Center, and the web sign-in page resolves a TOTP challenge and enrolls an authenticator app for either. SSO pools (OIDC or SAML) must stay `disabled` because their identity provider performs MFA. TOTP is the only second factor; SMS is not offered. `mfa.totp.adjacent_intervals` (an integer from 0 to 10, default 5, as [documented for TOTP MFA](https://firebase.google.com/docs/auth/web/totp-mfa)) is the number of adjacent 30-second steps GCIP accepts on either side of the current code to absorb clock skew; it is ignored while the Requirement is `disabled`.

```hcl
gcip_tenants = {
  acme = {
    tenant = "acme"
    mfa    = { state = "mandatory", totp = { adjacent_intervals = 2 } }
  }
}
```

The Google provider has no tenant MFA arguments ([terraform-provider-google#17912](https://github.com/hashicorp/terraform-provider-google/issues/17912)), so the module only outputs each Tenant ID's declared `{state, totp_adjacent_intervals}` and the controller owns the tenant's `mfaConfig` through [tenants.get](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/get) and [tenants.patch](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/patch) with `updateMask=mfaConfig`. `enabled` and `mandatory` hold `{state: ENABLED|MANDATORY, enabledProviders: [], providerConfigs: [{state: ENABLED, totpProviderConfig: {adjacentIntervals: N}}]}`; `enabledProviders` lists SMS factors only, so TOTP is switched on through its provider config. The controller also publishes the pools declared `mandatory` to the server as the runtime setting `mfa_mandatory`, which only lets the sign-in page lead a user with no second factor through enrollment before the exchange. GCIP's documented client flow lets a user with no enrolled factor sign in with valid first-factor credentials and leaves requiring enrollment to the app; whether a `mandatory` tenant already rejects that sign-in is unconfirmed (see the live check in the PR for #201), so the sign-in page leads such a user through enrollment before the exchange and Aidash does not otherwise enforce it, because it never reads second-factor claims as authority (the verified sign-in is identical with or without `firebase.sign_in_second_factor`).

The controller compares the normalized `mfaConfig` (state, enabled providers and enabled provider configs; a missing config is disabled) on every scheduled reconciliation for every retained environment, running or stopped, and before a host reopens admission inside each GCIP refresh and after each deployment apply. Drift is overwritten with the complete declared config, reported as a `GCIP MFA drift corrected` workflow warning naming the environment, Tenant ID and fields, and recorded as `gcip_mfa_drift_at` on the environment entry. Each correction is reported and recorded before the next tenant is checked, so a later tenant's failure never loses it. A failed or deadline-interrupted scheduled correction sets the pending marker, so the next run fences the host and retries through the GCIP refresh; a failed deployment-time correction keeps the deployment gated. MFA declarations are excluded from both GCIP digests: changing them never fences hosts. They have their own digest instead (`gcip_mfa_revision`, normalized to the defaults, so declaring `disabled` explicitly is no change): when it differs from the environment's recorded value, the controller runs an unfenced apply so the module output carries the new declaration, then the GCIP refresh enforces it on the tenant and republishes the `mfa_mandatory` hint, which travels in the runtime configuration beside `password_sign_up`, reloading a running host. Environments whose Terraform state predates the `mfa` output are reconciled from their next apply, before a deployment that performs it unseals the host.

`roles/identityplatform.viewer` includes `firebaseauth.users.get` in the [official permission list](https://docs.cloud.google.com/iam/docs/roles-permissions/identityplatform). Its applicability and effective isolation of tenant `accounts:lookup` remain **documented assumptions awaiting a sandbox test**. Verify own-pool lookup succeeds and another environment's pool and project-root lookup fail before rollout; failure must stop rollout and return to the lead, never broaden runtime IAM. The trusted deploy principal holds Identity Platform administration to create pools and reconcile their policies. Password-change/reset behavior for `validSince` also awaits sandbox verification; the server relies only on the returned timestamp.

Provider-mocked Terraform cases cover multi-tenancy, callback domains, per-pool signup, IdP resources and the MFA Requirement default, TOTP window range and SSO restriction. Fake REST controller tests cover grant creation, repeated reconciliation, unrelated bindings, etag retry, destruction, and tenant MFA drift correction (including the complete `enabled`/`mandatory` TOTP config), recording and pending retry. Rust and Playwright use signed fixtures; the Firebase Auth Emulator is not used.
