# GCP staging and test environments

This provisions nonproduction GCP environments using Terraform, a trusted
GitHub Actions controller, and infrastructure-side
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
`us-central1-a`. PostgreSQL 17 with pgvector/PGroonga, NATS, web/API/workers, K3s and the existing
gVisor Runner run on that host. Execution uses disposable Pods without a new
global Python/Shell queue. The existing runtime still controls physical writer
freeze, termination proof and uncertain-operation recovery.

Only one instance of each environment kind may run; other PR requests wait for
the preview slot. Stopped PRs retain their own independent data. `stop` retains
disks and identity; `destroy` deletes that environment's disks and secret. A
closed/merged PR is destroyed by reconciliation, regardless of its build result.
Reopening it requires a fresh explicit request.

The preview hostname has a separate 10 GiB `aidash-preview-tls` disk managed by
`environments/`, with Terraform destruction protection. Only the running preview
attaches it; stop detaches it before another PR can use the slot. Caddy mounts it
at `/var/lib/aidash/tls`, preserving certificates and ACME accounts across PR
replacement and retirement. It contains no application data and remains allocated
after the last PR is destroyed. Develop/test keep TLS state on their own data disks.
Do not delete the shared store during routine PR cleanup.

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

All five service images are digest-addressed in the private registry. NATS uses a reviewed upstream linux/amd64 digest in `control/images.py`, passes
through the same build-archive/publisher boundary and appear in the release
manifest. Retried workflows re-emit a pending build for the accepted SHA and
generation; newer stop/destroy requests still fence it. Attempt-specific publish
tags and replaceable workflow artifacts allow recovery from partial publication.

Bootstrap compares installed K3s and all gVisor binaries with the pinned hashes,
repairs or upgrades retained installations, and restarts K3s when binaries or
runtime configuration changed. After readiness succeeds, host health checks
remove obsolete Aidash image digests. They preserve the current observer and
sandbox images, images used by Pods/containers and unrelated repositories.

Runtime SAs can read image/bundle artifacts and only their own runtime secret.
They do not receive DNS or deployment credentials. Only ports 80/443 are public;
SSH uses IAP. Database, probe, proxy-control and Runner endpoints are loopback
only. Sandbox Pods retain deny-all networking and no host credentials. The
existing Runner must prove isolation and Python freeze/termination at startup
before an environment is declared ready.
Authentication throttling trusts only the loopback Nginx peer's `X-Real-IP`.
Caddy replaces client-supplied forwarding headers with its socket client address,
and Nginx overwrites `X-Real-IP`; separate clients therefore keep separate budgets.

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

Without GCIP, each runtime JSON contains string values for `AIDASH_OIDC_CLIENT_ID` and
`AIDASH_OIDC_CLIENT_SECRET`, plus required provider credentials named
`AIDASH_SECRET_*`. Optional Google session lifetime settings are
`AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS` and `AIDASH_OIDC_SESSION_IDLE_SECONDS`.
When GCIP is enabled, omit all `AIDASH_OIDC_*` keys. The controller adds the
public `dashboard.gcip` fragment to the same JSON; provider credentials remain
flat `AIDASH_SECRET_*` strings. The host writes only the GCIP fragment into a
read-only mounted settings directory and selects it through `AIDASH_GCIP_SETTINGS`.
Reinhardt composes this source with the normal server settings, applies typed
defaults and validates the sole issuer. The host does not export legacy OIDC
settings in this mode. Removing Tenant Bindings updates the retained settings file
before the environment is reopened.
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
No environment is created by opening a PR alone. Command acceptance and cloud
authentication require a complete preview command beginning with `/preview `
and current repository write/maintain/admin permission. A separate intake job
has no cloud or OIDC permissions; ordinary issue/PR comments and unrelated
authors are excluded before it starts, and malformed/unauthorized commands
cannot start the prepare/apply jobs.

Command acceptance and cloud
reconciliation share a durable lock, so an older apply cannot create/start a VM
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
interrupted/failed state, source and digests. A build artifact alone is not proof
of a successful deployment.

A newly created, resumed or replaced VM runs the Compute Engine startup script
once at boot. The controller waits for that service to finish; only a failed
initial run gets one explicit retry after credentials are provisioned. An update
to a running VM waits for any existing bootstrap before explicitly restarting it.
A timeout leaves the generation failed for operator inspection, without replaying
an install whose outcome is unknown.

## Idle stop, updates and failure recovery

The proxy counts admitted login/submission/save/execution requests, including
in-flight writes not yet visible in PostgreSQL. Polling, health checks, reading,
scrolling and unsent drafts do not renew the deadline. A host timer samples
durable work every minute. The read-only observer covers Runs, leases, capability
operations, verification, generation, transactions and active transfers; Runner
journals cover uncertain physical writers. Federation writes, including bodies
still being uploaded, block sealing and successful completion renews activity.
Completed work and explicit resume renew the idle deadline. Transfer receipts
have no completion timestamp: a newly observed inbound/outbound completion starts
a full idle hour at observation time, and subsequent samples of that receipt do
not extend it. Pure human-input/approval waits may idle; approval
expiration remains wall-clock based.
An update waiting for a failed, cancelled or unfinished build still observes its
previously applied VM and can stop it when idle. The pending source remains
recorded, but a late build cannot attach to or wake the stopped environment.

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
- **Abandoned lifecycle lock:** read `lifecycle/apply.lock`, inspect its GitHub run
  and confirm that no intake or reconciliation still runs before removing that exact GCS generation.
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

The combined budget target is JPY 10,000/month, including VM runtime, retained
disks, IPv4, registry/bundle/state storage, traffic, secrets, logs, applicable
Actions charges and tax; LLM/API costs are separate. This target is not a billing cap.
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

Provider Credential Key Material uses a separate existing, billing-enabled BYOK
project. Set `byok_project_id` explicitly in bootstrap and deployment configuration
to enable BYOK provisioning; it must differ from `project_id` and contain only
Provider Credential secrets. Omit it or leave it empty for existing deployments
without the Store: no BYOK API, project lookup, role, audit configuration or runtime
grant is provisioned. Existing shared-project lifecycle operations remain available.
Before clearing or replacing an enabled `byok_project_id`, retire all managed
environments using the applied project. The controller refuses a mismatched
project before observing hosts or applying Terraform while environment inventory
remains; cleanup must verify each old prefix is empty before removing its runtime
identity and disk. Enabling BYOK on a legacy deployment remains supported.

Bootstrap enables Secret Manager and its DATA_READ/DATA_WRITE audit logs there.
Human-run bootstrap defines `aidashByokCreate`, `aidashByokManage` and
`aidashByokBrokerRead` once per deployment. Environment automation binds runtime
service accounts to the create-only role without a condition and the seven-permission
manage role with their environment's Secret name prefix using the numeric project
number. Runtime identities have no BYOK payload access or IAM-setting permission.
The shared project's existing per-secret runtime configuration read remains in
place for host startup.

BYOK-enabled VM metadata supplies the non-secret Store descriptor through
`aidash-provider-credentials`. Host startup mounts a descriptor-only JSON source
read-only into migrations and the app; Reinhardt loads it via
`AIDASH_PROVIDER_CREDENTIAL_SETTINGS`. The runtime configuration secret must also
include a stable independent `AIDASH_PROVIDER_FINGERPRINT_KEY` of at least 32
bytes. Startup refuses a missing or short key; deployments never regenerate it.
Keep that key unchanged across upgrades and restarts. It stays in restrictive
`app.env`, never in VM metadata or the public descriptor. Environments
provisioned with the earlier `AIDASH_SECRET_PROVIDER_FINGERPRINT` name keep
working: host startup uses that value as `AIDASH_PROVIDER_FINGERPRINT_KEY` when
the new name is absent and never passes the legacy name to the app. Move the
value to the new name at the next runtime secret update. Disabled BYOK omits the
metadata attribute, renders no Store, and needs no fingerprint key. The managed descriptor uses `fingerprint_key = {env = "AIDASH_PROVIDER_FINGERPRINT_KEY"}`
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

Before deleting an environment's retained disk, identity or runtime IAM,
retirement collects every inventory page and deletes only names starting with
`aidash-<environment_id>-cred-`, then confirms that prefix is empty. Already
deleted secrets are skipped on retry. Existing running app writers are sealed
first; stopped or missing VMs are cleaned without a wake or recreation. Failure
keeps the retained disk and identity for retry. This does not add effective
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

The controller records digests of both desired shared inputs (Tenant configuration, IdP secrets and web API key) and actual public GCIP outputs, including generated Tenant IDs. Before applying changed shared inputs or a plan that changes GCIP tenant resources, it closes proxy admission, pauses the application and stops the trusted runner on every affected running host, including unpublished hosts. The fence uses the retained host's existing lifecycle lock, state and command API. It never calls an HTTP endpoint on a paused or absent application and requires confirmed quiescence before Terraform can apply.

After every apply, including unrelated lifecycle work, the controller compares actual GCIP outputs and refreshes IAM and runtime secrets for changed retained environments. Published hosts reload their existing authorized release with the new policy before reopening admission. Stopped environments receive the new secret without being started. Unpublished hosts remain fenced until an authorized deployment succeeds. Failed refreshes retain a pending marker for scheduled retry; successful environments are not restarted again on that retry. A fenced host cannot pass through ordinary seal/unseal rollback with its old policy: non-forced stop or redeployment waits for policy recovery, while the existing explicit force operation can stop the VM or repair the deployment. No IdP secret or unhashed private input is copied into lifecycle state.

The Google 7.46.1 provider has no tenant IAM resource. The approved controller adapter therefore calls [tenant getIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/getIamPolicy) and [tenant setIamPolicy](https://docs.cloud.google.com/identity-platform/docs/reference/rest/v2/projects.tenants/setIamPolicy). It adds `roles/identityplatform.viewer` only on that environment's tenant resources for that environment's runtime principal, preserving unrelated/conditional bindings and policy metadata, using etag concurrency and bounded conflict retries. Retirement removes only its managed viewer member before Terraform destroys the environment. Runtime principals receive no project-wide Firebase user access.

`roles/identityplatform.viewer` includes `firebaseauth.users.get` in the [official permission list](https://docs.cloud.google.com/iam/docs/roles-permissions/identityplatform). Its applicability and effective isolation of tenant `accounts:lookup` remain **documented assumptions awaiting a sandbox test**. Verify own-pool lookup succeeds and another environment's pool and project-root lookup fail before rollout; failure must stop rollout and return to the lead, never broaden runtime IAM. The trusted deploy principal holds Identity Platform administration to create pools and reconcile their policies. Password-change/reset behavior for `validSince` also awaits sandbox verification; the server relies only on the returned timestamp.

Provider-mocked Terraform cases cover multi-tenancy, callback domains, per-pool signup and IdP resources. Fake REST controller tests cover grant creation, repeated reconciliation, unrelated bindings, etag retry, and destruction. Rust and Playwright use signed fixtures; the Firebase Auth Emulator is not used.
