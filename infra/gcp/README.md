# GCP staging and test environments

This provisions nonproduction GCP Environments on one shared GKE Standard
cluster using Terraform, a trusted GitHub Actions controller, Helm and
infrastructure-side activity observation. Application and Runner business logic
are unchanged. No production Environment or always-running management VM is
created; with every Environment stopped, no Cluster Node runs.

## Environments and lifecycle

| Environment | Source                                                           | Node pool       | Public origin                |
| ----------- | ---------------------------------------------------------------- | --------------- | ---------------------------- |
| `develop`   | Configured `develop/x.y.z`                                       | Normal          | `https://develop.aidash.run` |
| `pr-N`      | Explicitly requested open PR targeting `main` or `develop/x.y.z` | Spot by default | `https://preview.aidash.run` |
| `test`      | Explicit branch or full SHA, then pinned                         | Spot by default | `https://test.aidash.run`    |

All Environments share the zonal GKE Standard cluster `aidash` in
`us-central1-a` (Dataplane V2, Workload Identity, private Cluster Nodes with Cloud
NAT, Kubernetes 1.34 or later). Each Environment owns:

- namespaces `aidash-<id>` (app), `aidash-<id>-sandbox` (Execution Pods) and
  `aidash-<id>-trusted` (guard/installer);
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
upstream gVisor installed by the privileged installer DaemonSet.

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
record. The preview hostname keeps a separate 10 GiB `aidash-preview-tls` GCE disk
managed by `environments/` with Terraform destruction protection. The controller
exposes it as the static PV `aidash-preview-tls` and binds it to the active
`aidash-pr-N/preview-tls` claim only after the previous PR's edge has no Pods and
no VolumeAttachment for it; until then the request waits as
`waiting_for_preview_tls`. Caddy mounts it at `/var/lib/aidash/tls`, preserving
certificates and ACME accounts across PR replacement and retirement. Destroying a
PR releases the claim reference and never deletes the disk. Develop/test keep TLS
state on their own retained claims.

Push/CI completion never creates a missing Environment or wakes a stopped one.
An active shared develop or opted-in same-repository PR follows its current,
CI-successful SHA. Test environments remain pinned. Updates wait for active
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

WIF trusts numeric repository/owner IDs and only
`.github/workflows/gcp-environments.yml` on `refs/heads/main`. The workflow
checks out privileged code and chart templates from the repository default
branch. Source images (`app`, `postgres`, `sandbox`) are built from an exact SHA
in a separate job without cloud credentials, using trusted Dockerfiles.
Privileged images (`observer`, `control` for the Runner/guard/installer and
`edge`) are built only from the trusted checkout, including for fork previews.
NATS and Caddy use reviewed upstream linux/amd64 digests in `control/images.py`.
A publisher job copies all eight image archives to digest-addressed private
registry entries without executing them; `release.json` records each digest.
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
   names. Use an operator identity allowed to enable APIs, create the bootstrap
   resources and grant their IAM roles. Authenticate Terraform using ADC, for
   example `gcloud auth application-default login`. Select `develop_branch`.
2. Put this implementation on the repository's default branch (`main`) through
   the normal review process. Select a deployable source containing Google OAuth,
   trusted-proxy authentication rate limits, the existing runtime contract and
   successful `ci.yml` for its **exact SHA**. The app build explicitly refuses
   sources missing either HTTP integration. `main` being the trusted automation branch does
   not create a main production environment.
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
6. Configure a Google Web OAuth client with exact redirect URIs:
   `https://develop.aidash.run/auth/callback`,
   `https://preview.aidash.run/auth/callback`,
   `https://test.aidash.run/auth/callback`.
   Configure the corresponding authorized origins and consent-screen test users.
   Google login authenticates identity; existing Aidash registration approval,
   member mapping and workspace permissions still control access. Set up the
   initial operator/invited users using the deployed Google integration's
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

The optional `node_count` map in `AIDASH_GCP_CONFIG` (keys `develop`, `test`,
`pr`) sets the Environment node pool size per kind; omitted kinds use one node.

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

The controller generates private database/API/Runner keys once into the
namespace's `aidash-identity` Secret and never regenerates them; callers cannot
override these or the node identity through runtime JSON. Terraform creates secret
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
a ready Runner and the public HTTPS node identity. Only then is the edge
LoadBalancer IP published in DNS and admission opened. A timeout leaves the
generation failed for operator inspection, without replaying unknown work.

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

The stop threshold is 60 minutes. Reconciliation is scheduled every five minutes
and GitHub scheduling/startup can be delayed; **this is not an exact shutdown-time
SLA**. PR-close cleanup has the same scheduling delay. If Actions is disabled or
observation/IAM fails, an Environment can keep accruing charges; inspect failed
runs. No idle timer resumes a stopped Environment.

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
Backups and production availability are deferred.

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
[broker deployment guide](../../crates/aidash-broker/README.md) for the input
contract and operator-only import steps for any pre-merge draft deployment.

The existing deployment identity has shared-project
`roles/iam.serviceAccountAdmin`, allowing it to change the IAM policy of any
broker service account placed there and grant itself impersonation permission.
An identity allowed to redeploy a future broker can also execute broker code
that reads Key Material. The BYOK role-grant restriction does not close these paths.
The deploy pipeline remains part of the v1 trust root. The human-run IAM deny
policy to block broker impersonation and deployment entry-point hardening are
tracked in [#151](https://github.com/kent8192/aidash/issues/151). A separate
credential enclave is deferred. The existing shared-project deploy grant is
retained here; no read identity is attached to it by this change.

## GCIP sign-in

Enable `gcip_enabled` in bootstrap and supply `environment_domains` matching the environment stacks' `domain` values. Bootstrap enables Identity Platform multi-tenancy and derives the develop, preview and test authorized callback hosts, including the project's default Firebase auth domain. Export the sensitive `gcip_web_api_key` output into the public SDK configuration field; it is not an Admin API credential.

Supply `gcip_tenants` to the environment controller configuration. For example, `{"acme":{"tenant":"acme","password_sign_up":true,"google_client_id":"CLIENT_ID"}}` creates a fresh pool per environment incarnation. OIDC and SAML provider maps use `oidc.*` and `saml.*` IDs; those SSO pools must disable password signup. OAuth client secrets are separate `gcip_idp_secrets` Terraform inputs, supplied by `AIDASH_GCIP_IDP_SECRETS` in the controller (the lifecycle workflow reads the repository secret `GCP_GCIP_IDP_SECRETS` as this JSON map). They are confined to private inputs and protected Terraform state. Each module outputs its GCIP Tenant IDs, runtime service-account email, Tenant Bindings, providers and password-signup list. The controller merges these settings into the environment runtime secret, preserving its other settings and refusing coexistence with OIDC.

The controller records digests of both desired shared inputs (Tenant configuration, IdP secrets and web API key) and actual public GCIP outputs, including generated Tenant IDs. Before applying changed shared inputs or a plan that changes GCIP tenant resources, in every affected running Environment, including unpublished ones, it closes edge admission and scales server, worker and Runner to zero under the existing lifecycle lock and state. It never calls an HTTP endpoint on a paused or absent application and requires confirmed quiescence before Terraform can apply.

After every apply, including unrelated lifecycle work, the controller compares actual GCIP outputs and refreshes IAM and runtime secrets for changed retained environments. Published Environments upgrade the `app` release of their existing authorized source with the new policy before reopening admission. Stopped environments receive the new secret without being started. Unpublished Environments remain fenced until an authorized deployment succeeds. Failed refreshes retain a pending marker for scheduled retry; successful environments are not restarted again on that retry. A fenced Environment cannot pass through ordinary seal/unseal rollback with its old policy: non-forced stop or redeployment waits for policy recovery, while the existing explicit force operation can stop the Environment or repair the deployment. No IdP secret or unhashed private input is copied into lifecycle state.

The Google 7.46.1 provider has no tenant IAM resource. The approved controller adapter therefore calls [tenant getIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/getIamPolicy) and [tenant setIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/setIamPolicy). It adds `roles/identityplatform.viewer` only on that environment's tenant resources for that environment's runtime principal, preserving unrelated/conditional bindings and policy metadata, using etag concurrency and bounded conflict retries. Retirement removes only its managed viewer member before Terraform destroys the environment. Runtime principals receive no project-wide Firebase user access.

`roles/identityplatform.viewer` includes `firebaseauth.users.get` in the [official permission list](https://docs.cloud.google.com/iam/docs/roles-permissions/identityplatform). Its applicability and effective isolation of tenant `accounts:lookup` remain **documented assumptions awaiting a sandbox test**. Verify own-pool lookup succeeds and another environment's pool and project-root lookup fail before rollout; failure must stop rollout and return to the lead, never broaden runtime IAM. The trusted deploy principal holds Identity Platform administration to create pools and reconcile their policies. Password-change/reset behavior for `validSince` also awaits sandbox verification; the server relies only on the returned timestamp.

Provider-mocked Terraform cases cover multi-tenancy, callback domains, per-pool signup and IdP resources. Fake REST controller tests cover grant creation, repeated reconciliation, unrelated bindings, etag retry, and destruction. Rust and Playwright use signed fixtures; the Firebase Auth Emulator is not used.
