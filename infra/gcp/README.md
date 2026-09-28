# GCP staging and test environments

This implements the [accepted design](../../docs/design/2026-09-27-gcp-deployment.md)
using Terraform, a trusted GitHub Actions controller, and infrastructure-side
activity observation. Application and Runner business logic are unchanged.
No production environment or always-running management VM is created.

## Environments and lifecycle

| Environment | Source                                                           | Host            | Public origin                |
| ----------- | ---------------------------------------------------------------- | --------------- | ---------------------------- |
| `develop`   | Configured `develop/x.y.z`                                       | Normal VM       | `https://develop.aidash.run` |
| `pr-N`      | Explicitly requested open PR targeting `main` or `develop/x.y.z` | Spot by default | `https://preview.aidash.run` |
| `test`      | Explicit branch or full SHA, then pinned                         | Spot by default | `https://test.aidash.run`    |

Each environment owns a VPC, service account, Secret Manager secret, VM,
10 GiB boot disk and 20 GiB data disk. The initial host is `e2-standard-4` in
`us-central1-a`. PostgreSQL 17, NATS, Qdrant, web/API/workers, K3s and the existing
gVisor Runner run on that host. Execution uses disposable Pods without a new
global Python/Shell queue. The existing runtime still controls physical writer
freeze, termination proof and uncertain-operation recovery.

Only one instance of each environment kind may run; other PR requests wait for
the preview slot. Stopped PRs retain their own independent data. `stop` retains
disks and identity; `destroy` deletes that environment's disks and secret. A
closed/merged PR is destroyed by reconciliation, regardless of its build result.
Reopening it requires a fresh explicit request.

Push/CI completion never creates a missing environment or wakes a stopped host.
An active shared develop or opted-in same-repository PR follows its current,
CI-successful SHA. Test environments remain pinned. Updates wait for active
work and coalesce to the latest authorized source. Explicit normal-VM overrides
persist across updates. Spot preemption/unavailability requires manual resume;
there is no automatic normal fallback or infrastructure replay of tool code.

## Structure and trust boundary

- `bootstrap/`: shared private state/release buckets, Artifact Registry, APIs,
  deployment/publisher identities and GitHub Workload Identity Federation.
- `environments/`, `modules/environment/`: retained per-environment resources and
  the three DNS-only Cloudflare A records. The existing DNS zone is not owned.
- `control/`: authorized requests, generation fencing, image publication,
  desired-state reconciliation and explicit Compute Engine power operations.
- `runtime/`: VM setup, reverse proxy, admission control and host observation.
- `observer/`: separate Rust program; SeaQuery/SeaORM, read-only transactions,
  no changes to the application schema.
- `tests/`, `check.sh`: provider-mocked Terraform tests, controller/host regressions,
  real PostgreSQL queries and a real Ubuntu/Nginx admission fixture.

WIF trusts numeric repository/owner IDs and only
`.github/workflows/gcp-environments.yml` on `refs/heads/main`. The workflow
checks out privileged code from the repository default branch. Source images
are built from an exact SHA in a separate job without cloud credentials, using
trusted Dockerfiles. A publisher job copies image archives to digest-addressed
private registry entries without executing them. Fork heads require explicit
write-collaborator authorization for that exact SHA; a retained approval covers
resuming the same SHA, never a later commit. Actions/comments require repository
write, maintain or admin permission. No workflow posts PR comments.

Runtime SAs can read image/bundle artifacts and only their own runtime secret.
They do not receive DNS or deployment credentials. Only ports 80/443 are public;
SSH uses IAP. Database, probe, proxy-control and Runner endpoints are loopback
only. Sandbox Pods retain deny-all networking and no host credentials. The
existing Runner must prove isolation and Python freeze/termination at startup
before an environment is declared ready.

## Initial configuration

Provisioning is intentionally disabled until these account-specific inputs are
configured. The project is never inferred from a developer's `gcloud` default.

1. Choose a billing-enabled **Aidash GCP project** and globally unique bucket
   names. Use an operator identity allowed to enable APIs, create the bootstrap
   resources and grant their IAM roles. Authenticate Terraform using ADC, for
   example `gcloud auth application-default login`. Select `develop_branch`.
2. Put this implementation on the repository's default branch (`main`) through
   the normal review process. Select a deployable source containing Google OAuth,
   the existing runtime contract and successful `ci.yml` for its **exact SHA**.
   The implementation base `dcede2adb3403fb281c1efaf3c889870521457b5` does not
   contain the separately developed Google integration; the app build explicitly
   refuses sources missing it. `main` being the trusted automation branch does
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
   A/AAAA/CNAME records. No Cloud DNS zone, paid load balancer or static IP is used.
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

Each runtime JSON contains string values for `AIDASH_OIDC_CLIENT_ID` and
`AIDASH_OIDC_CLIENT_SECRET`, plus required provider credentials named
`AIDASH_SECRET_*`. Optional Google session lifetime settings are
`AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS` and `AIDASH_OIDC_SESSION_IDLE_SECONDS`.
The host generates private database/API/Runner keys and environment-specific
node identity; callers cannot override these through runtime JSON. Terraform
creates secret metadata only. The controller uploads an initial secret version
via stdin; the host reads it using its own service account. Runtime files are
root-readable and secret values do not enter Terraform state.

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
No environment is created by opening a PR alone. Commands and completed builds
store durable intent before acquiring the apply lock. If another apply owns the
lock, a later scheduled reconciliation processes that intent. Inspect Actions
logs and `lifecycle/state.json` in the private state bucket for ready/pending/
interrupted/failed state, source and digests. A build artifact alone is not proof
of a successful deployment.

## Idle stop, updates and failure recovery

The proxy counts admitted login/submission/save/execution requests, including
in-flight writes not yet visible in PostgreSQL. Polling, health checks, reading,
scrolling and unsent drafts do not renew the deadline. A host timer samples
durable work every minute. The read-only observer covers Runs, leases, capability
operations, verification, generation, transactions and active transfers; Runner
journals cover uncertain physical writers. Completed work and explicit resume
renew the idle deadline. Pure human-input/approval waits may idle; approval
expiration remains wall-clock based.

Before stop/update, the controller closes proxy admission, freezes application
and Runner controllers and checks work again, including outstanding HTTP and
database transactions. If work remains or observation fails it restores admission
and defers. Explicit `force=true` permits interrupting work, including repair of
an incomplete bootstrap. An uncertain operation is never automatically replayed
by infrastructure. A normal resume of an already running host renews its idle hour.

The stop threshold is 60 minutes. Reconciliation is scheduled every five minutes
and GitHub scheduling/startup can be delayed; **this is not an exact shutdown-time
SLA**. PR-close cleanup has the same scheduling delay. If Actions is disabled or
observation/IAM fails, a host can keep accruing charges; inspect failed runs. No
idle timer can automatically resume a stopped VM.

- **Spot unavailable/interrupted:** inspect the failure, then explicitly resume
  or choose `vm_mode=normal`. The power authorization is consumed once. Terraform
  ignores drift in `desired_status`, so another environment's apply cannot undo
  preemption. Missing VMs are recorded without automatic replacement.
- **Bootstrap/migration/readiness failure:** data is retained and DNS withdrawn;
  no destructive rollback or database downgrade is attempted. Repair the cause
  and use explicit `resume` (with `force=true` if admission observation is broken).
  A failed deployment generation is not retried by cron. HTTPS readiness checks
  the environment's node identity, not just an HTTP 200 from an old preview IP.
- **Runtime credential rotation:** add a new version to that environment's
  Secret Manager secret using secure file/stdin input, then explicitly resume
  with `force=true` to reinstall configuration. Changing a GitHub seed secret
  affects newly created environment secrets, not already populated secrets.
- **Abandoned apply lock:** read `lifecycle/apply.lock`, inspect its GitHub run
  and confirm that no apply still runs before removing that exact GCS generation.
  Locks are never stolen on a timeout. Preserve `lifecycle/state.json`, including
  tombstones; deleting it loses stale-run fencing. Terraform's own backend lock
  is separate and also requires proving the owner has stopped before recovery.
- **Partial Terraform apply:** inspect the plan/state and retained disk ownership
  before retrying. The controller rejects plans deleting any disk outside an
  explicit retirement. Missing canonical state output requires operator repair;
  do not reset state or remove disks to get a clean plan.

On a host, inspect protected `google-startup-scripts`, `k3s`, `aidash-runner` and
`aidash-node-guard` service journals through IAP. Avoid publishing configuration,
database URLs or logs containing private application data. Container and access
logs are rotated. Sandbox runtime paths match the existing node guard:
`/run/containerd/runsc/k8s.io` and `/var/lib/kubelet/pods`.

## Validation and remaining deployment gates

Run `bash infra/gcp/check.sh` with Terraform, Rust and Docker installed. It creates
only disposable local test containers and uses mocked cloud providers. Also run
Ruff, actionlint and `git diff --check` for changed Python/workflows. Main CI now
requires the reusable **GCP infrastructure checks** job.

The local checks do not establish real GCP IAM/boot, gVisor on the selected VM,
Google login, SSE, DNS/TLS, retained-data recovery or a measured capacity limit.
Before inviting users, exercise create, a real Shell/Python/file operation,
long-running-work drain, one idle hour, stop/resume, PR replacement/close, explicit
Spot recovery and deletion. Confirm Runner `verified` and `python_verified`,
Google registration/authorization, and retained data/identity. Measure memory,
disk growth and concurrent sandbox admission on the initial 4 vCPU/16 GiB host.
Source/host-Runner protocol compatibility and observer schema readiness are
explicit startup gates. Backups and production availability are deferred.

## Cost and external references

The [accepted cost worksheet](../../docs/design/2026-09-28-gcp-host-cost-estimate.md)
uses 60 normal + 60 Spot VM-hours/month, 90 GiB retained disks and ephemeral IPv4:
approximately JPY 3,571 for that subtotal at JPY 160/USD. An additional stopped
30 GiB PR adds about JPY 480/month. This is a dated estimate, not measured usage
or a cap. Add registry/bundle/state storage, traffic, secrets, logs, applicable
Actions charges and tax to the combined JPY 10,000 target; LLM/API costs are separate.
Retained images/bundles are not automatically expired because stopped environments
must remain resumable. Periodically remove only versions no longer referenced by
desired/applied lifecycle state. Review billing and artifact growth explicitly.

Scheduled workflows also consume Actions capacity even when hosts are stopped.
The repository was public when implemented; check current plan/billing terms
before changing visibility or runner type. No Cloud SQL, GKE management fee,
Cloud NAT, managed load balancer, Cloud DNS zone or permanently running VM is
included in this topology.

Implementation references: [K3s containerd templates](https://docs.k3s.io/advanced#configuring-containerd),
[gVisor containerd setup](https://gvisor.dev/docs/user_guide/containerd/quick_start/),
[Nginx Lua admission hooks](https://github.com/openresty/lua-nginx-module#access_by_lua_block),
[GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows),
[Google WIF](https://docs.cloud.google.com/iam/docs/workload-identity-federation-with-deployment-pipelines),
[Cloudflare DNS Terraform](https://developers.cloudflare.com/api/terraform/resources/dns/subresources/records/).
