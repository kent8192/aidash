# Release deployment operations

Keep deployment and notification enablement disabled until account setup is
complete. Enable nonproduction for a supervised acceptance window; keep
production disabled until its isolated readiness gates pass. Local tests establish controller,
workflow and Terraform contracts; they do not establish running infrastructure,
mail delivery, recovery objectives or hosted CI success.

## Activation checklist

Complete these steps in order. The Cloudflare sending domain already reports
**Configured**; its API credential and incoming forwarding aliases still need
manual setup and delivery verification.

Use [delivery setup automation](../../infra/delivery/README.md) for the commands,
private input templates and existing-resource adoption. These tools cover the
Terraform and credential handoff portions below; acceptance and approvals remain
explicit manual actions.

| Step | Automated setup                                                                                                                                                                                                                                    | Manual completion evidence                                                                                                                                                                                                                                                                       |
| ---- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1    | Four workflow enablement variables remain false                                                                                                                                                                                                    | Review/merge the privileged code into default develop first, then main with the first release; synchronize main history into develop                                                                                                                                                             |
| 2    | Five protected Environments, exact branch policies, additive CI/App checks; release App manifest and protected key handoff                                                                                                                         | Owner Create/Install consent for only Aidash; live reviewer/branch protections; enable the App staging check only after bootstrap main merge                                                                                                                                                     |
| 3    | Dedicated production project on the same billing account, two isolated bootstrap stacks, private state/release/notification buckets, exact WIF and staging read grants; public config composition and protected secret installation                | Supply separate runtime/IdP credentials and public GCIP pool definitions; verify cross-tier and unauthorized WIF denial                                                                                                                                                                          |
| 4    | TLS/ACME Secret metadata, dedicated renewal identity and daily DNS-01 certificate workflow; refresh running preview edges                                                                                                                          | Fill the scoped DNS token and ACME contact; verify wildcard issuance and external OAuth/SSO callbacks with authenticated access                                                                                                                                                                  |
| 5    | Discover configured Sending domain and verified destination, apply three Email Routing aliases and optional guarded Routing DNS; install the separate Sending token                                                                                | Fill account/token/private destination locally; verify that stg/prod/test mail reaches the intended inbox                                                                                                                                                                                        |
| 6    | Exact-CI staging and release automation, explicit `/preview up`, independent mail retry                                                                                                                                                            | Supervised nonproduction acceptance: random host lifetime, sign-in, Host/CSRF boundaries, fork rejection, stop/resume/retirement and actual stg/test mail                                                                                                                                        |
| 7    | Prepare Backup for GKE agent; after live cluster exists, apply daily volume/Secret backups, the Pre-update Backup and in-place restore plans, and production uptime/capacity/restart/backup-failure alerts; inspect backup freshness/configuration | Isolated restore drill including the independent Home ledger, approved RPO/RTO and maintenance/migration recovery, plus a zone failover drill (primary zone made unreachable; web/API and Runs recover in the other zone with no acknowledged write lost); configuration does not prove recovery |
| 8    | Exact main CI, original staged digest promotion and fixed production notification                                                                                                                                                                  | Explicitly enable production after readiness, human-merge the validated release PR, approve the Environment, verify HTTPS/sign-in/digests and actual prod mail                                                                                                                                   |

After each production release, merge main back into develop through review.
For PR environments, use the explicit comment commands below. Inspect failed
deployments before authorizing their retry; mail retries do not redeploy.

## Release flow

1. Merge an approved application PR into the configured protected
   `develop/x.y.z`. Capture that exact push SHA as **S**.
2. After CI succeeds for S, **GCP environments** deploys S to the shared
   `develop` staging environment. Newer development commits supersede older
   candidates; a successful older run cannot publish readiness for a newer one.
3. In parallel, **Release PR** creates a Draft PR from the immutable
   `release/<UTC timestamp>-<S prefix>` branch into `main`, using the release
   GitHub App. One current managed release PR remains open. A newer candidate
   closes only older managed PRs and never rewrites their branches.
4. Verify staging behavior and the candidate SHA. The App publishes **Staging
   deployment** only after validating the original sanitized staging artifact,
   exact source CI and current protected development head. It then marks the
   matching Draft PR ready. Require **CI Success** and **Staging deployment**
   from their expected integrations before a human merges the release PR.
5. Production accepts the actual current protected `main` merge SHA **M** only
   after its exact CI succeeds. It verifies the human-merged managed release PR,
   staging evidence and `tree(M) == tree(S)`, then deploys the original tested
   eight-image digest bundle. A successful production readiness check queues
   the production notification.

The CI-main completion dispatcher has GitHub Actions write permission and no
cloud credentials. It dispatches **GCP production** on `main` with the exact
`main_sha` input after checking trusted CI identity, push/main provenance and
current protected head. Only that workflow's `workflow_dispatch` jobs on main
can obtain production WIF credentials. A five-minute schedule also dispatches
main to resume an already accepted intent waiting for active work. It repeats
a completed same-main run only when that successful run published a sanitized
`production-progress` artifact with explicit pending status. The artifact
contains only `protocol: aidash-production-progress/1`, `main_sha` and `status`.
Ready, failed, missing/invalid artifacts and failed workflows suppress further
automatic dispatches and approvals; uncertain outcomes require manual recovery.
The same accepted main SHA retains its generation, original tested bundle and
authorization while waiting, even if the latest staging index changes or the
evidence expires. The schedule never retries failed migrations or execution.

S and M usually differ because a merge creates a new commit. Do not rebuild
images merely to relabel S as M. Record S, M and the bundle's trusted controller
SHA **T** separately. T equals S, because nonproduction trusts the protected
development branch head it deploys; production refuses any bundle whose T is not
S. The bundle includes app, postgres, sandbox, observer, nats, control, edge and caddy.
Fresh production admission requires staging evidence no older than seven days
(`staging_max_age_seconds` defaults to 604800). Revalidate expired evidence; do
not edit a stored receipt. Each validation writes an immutable archive at
`deployments/develop/<S>/<event_id>-<validation_run>-<validation_time>.json` and advances the CAS-protected latest
index at `deployments/develop/<S>.json`, allowing the same source to be tested
again. The public artifact contains only protocol, source SHA, trusted SHA,
generation, status, eight image digests and an integer run ID; it strips the
private evidence timestamp and all URLs. If receipt publication fails after
staging is already ready, recovery checks the current source against Kubernetes
and public HTTPS before publishing evidence, without redeploying. The successful
validation/publication run is recorded separately from the original stable
notification run, and the recovery run uploads the sanitized staging artifact.
A main hotfix or divergent merge requires synchronization and a fresh candidate
matching the prospective main tree.

After each release merge, merge `main` back into `develop/x.y.z` through the
normal reviewed workflow before the next release candidate. The automation
requires the current main history to be an ancestor of S. Keep the App-created
branch and provenance marker unchanged, and merge through a human account with
repository write permission. Automatic or unrelated main merges do not qualify
for production.

PR deployments remain explicit: an authorized collaborator comments
`/preview up`, optionally followed by `normal` or `spot`. A fork additionally
requires the reviewed full head SHA, for example
`/preview up normal <40-character SHA>`. Only write, maintain or admin permission
can authorize this operation. `/preview stop` preserves the environment;
`/preview destroy` removes its owned data. These actions cannot operate production.

## Release PR diagnostics

The Release PR workflow records JSON events for configuration validation,
candidate validation and creation, staging receipt discovery and validation,
and readiness publication. Each phase reports its outcome and elapsed time;
failures include the exception type and source line without exception text.
An always-run final step reports checkout, configuration, App authentication
and controller outcomes, including failures before the controller starts.

Each GitHub request records its method, route template, request sequence and
elapsed time. Responses include the HTTP status, GitHub Request ID and available
rate-limit/retry metadata. Known API and transport failures receive fixed
categories such as `integration_permission_denied`, `no_commits_between` or
`connection_reset`. Validation details include only allowlisted resource,
field, code and GraphQL error types. Unknown errors retain the status and
Request ID with `unclassified`; arbitrary server text remains withheld.

Configuration is passed to the reusable execution workflow as a secret so
GitHub masks it before printing any step environment. The existing
`release-automation` Environment still supplies the App private key. Logs never
include request/response bodies, GraphQL variables, configuration values,
credentials, arbitrary stderr, signed download URLs or archive contents.
The controller disables inherited `GH_DEBUG` output. Do not add `--verbose`,
shell tracing or environment dumps when investigating a failure. Use the
recorded UTC time, Request ID, source/controller SHA and phase to correlate
the failure with GitHub support and the exact trusted source revision.

## Account and GitHub setup

Publish the reviewed workflow and controller changes to the appropriate
protected branches before enabling them. `automation_branch` is the protected
repository default branch, currently `develop/0.1.0`, on which nonproduction
workflow-run, comment and scheduled automation executes. Nonproduction
privileged jobs pin one commit of that protected branch and never execute PR
code. `trusted_branch` stays `main` and now names only production's code anchor:
the production dispatcher and notifications check out `main`, and production
applies the checkout of S. Run manual **GCP environments** operations on `automation_branch`, not on the
source branch being deployed or on the trusted-code checkout ref. These names
have different purposes. WIF checks the workflow execution ref and
GitHub Environment, independently of the code checkout. Changing the default
branch requires coordinated workflow, GitHub Environment and WIF configuration.

Create separate bootstrap stacks for nonproduction and production. Production
must have its own GCP project, Terraform backend, release bucket, notification
bucket, cluster, runtime identities and credentials. Set
`deployment_tier = "production"`, explicit `nonproduction_project_id` and
`nonproduction_state_bucket`, and the nonproduction `staging_release_bucket`.
Production reads only the sanitized staging-evidence prefix and pulls the exact
staging image digests through explicitly granted Artifact Registry access.
Never copy nonproduction runtime credentials or application data into production.
Use the bootstrap outputs with `infra/gcp/config.example.json` and
`infra/gcp/config.production.example.json`; placeholders are not deployable inputs.

Protect these GitHub Environments before storing credentials. Keep production
runtime secrets in `production` and nonproduction runtime secrets in
`nonproduction`; configuration JSON and service-account identifiers are
repository variables so credential-free intake can validate them:

| Environment                | Allowed execution branch  | Purpose                                                                                |
| -------------------------- | ------------------------- | -------------------------------------------------------------------------------------- |
| `nonproduction`            | Exact `automation_branch` | Staging, test and explicitly authorized PR lifecycle                                   |
| `production`               | `main` only               | Isolated production deployment; configure required reviewers and prevent self approval |
| `release-automation`       | Exact `automation_branch` | Release GitHub App private key; no source builds or application runtime                |
| `deployment-notifications` | Exact `automation_branch` | Mail delivery with no application execution or deployment permissions                  |
| `certificate-renewal`      | Exact `automation_branch` | Scoped DNS-01 and TLS/ACME Secret versions; no application deployment                  |

Require protected `main` and `develop/x.y.z`, reviewed changes, and the CI checks.
Require **Staging deployment** on release PRs specifically from the release App,
not any actor that can create a status with that name. Install that App only on
this repository, with Actions read, Contents write, Pull requests write and
Checks write. Set `RELEASE_APP_CLIENT_ID` as a repository variable and
`RELEASE_APP_PRIVATE_KEY` only in the protected `release-automation` Environment,
not as a repository secret. Set matching `release_app_bot_login` in both config
JSONs. The App token is short lived and
is never passed to source builds, runtime Pods or receipts.

| Scope                                             | Variables                                                                                                                                                           | Secrets                                                                                                                              |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Repository / nonproduction                        | `AIDASH_GCP_CONFIG`, `GCP_WORKLOAD_IDENTITY_PROVIDER`, `GCP_DEPLOY_SERVICE_ACCOUNT`, `GCP_PUBLISH_SERVICE_ACCOUNT`, `GCP_NOTIFICATION_SERVICE_ACCOUNT`              | `CLOUDFLARE_DNS_API_TOKEN`, `GCP_DEVELOP_RUNTIME_CONFIG`, `GCP_TEST_RUNTIME_CONFIG`, `GCP_PR_RUNTIME_CONFIG`, `GCP_GCIP_IDP_SECRETS` |
| Repository / production                           | `AIDASH_GCP_PRODUCTION_CONFIG`, `GCP_PRODUCTION_WORKLOAD_IDENTITY_PROVIDER`, `GCP_PRODUCTION_DEPLOY_SERVICE_ACCOUNT`, `GCP_PRODUCTION_NOTIFICATION_SERVICE_ACCOUNT` | `GCP_PRODUCTION_RUNTIME_CONFIG`, `GCP_PRODUCTION_GCIP_IDP_SECRETS`, `CLOUDFLARE_DNS_API_TOKEN`                                       |
| `deployment-notifications` Environment            | `CLOUDFLARE_ACCOUNT_ID`                                                                                                                                             | `CLOUDFLARE_EMAIL_SENDING_TOKEN`                                                                                                     |
| Repository variable / `release-automation` secret | `RELEASE_APP_CLIENT_ID`                                                                                                                                             | `RELEASE_APP_PRIVATE_KEY`                                                                                                            |

Use the private input/CLI helper to configure Environment secret values through
stdin. Keep payloads out of shell arguments, Terraform-managed GitHub resources,
Git history, workflow artifacts and chat. The Cloudflare bootstrap token is an
ephemeral Terraform input in a private ignored file; its forwarding destination
is sensitive state and requires a restricted backend. Leave
`GCP_ENVIRONMENTS_ENABLED`, `GCP_PRODUCTION_ENABLED`,
`GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED` and `GCP_CERTIFICATE_RENEWAL_ENABLED` false
during account setup, then enable
them only in the supervised sequence above. The release-flow config must also
explicitly enable `release_flow_enabled`.

## Random preview hosts and sign-in

Staging and PR URLs use a controller-generated 128-bit random token:
`<32 lowercase hexadecimal characters>.preview.aidash.run`. The hostname stays
stable through updates and stop/resume within one environment incarnation.
Destroy/recreate issues a new token; remove the old DNS record and exact GCIP
authorized domain instead of reassigning it to another PR. Test retains
`test.aidash.run`; production uses the explicitly configured production origin.
Legacy saved develop/PR environments use `legacy_hostname` to retain their old
fixed hostname safely for stop/destroy plans. Their next verified deployment,
including resume/up, migrates to a random token; unrelated legacy environments
do not block another deployment or teardown. Do not manually inject or reuse tokens.

Bootstrap Terraform creates metadata for `aidash-preview-wildcard-tls` and
`aidash-preview-acme-state`, plus a dedicated `aidash-tls-renewer` identity and
exact certificate-workflow WIF provider. Only this identity can read/list/add
versions of these two Secrets; it receives no application deployment capability.
PEM and ACME account private-key versions never enter Terraform state.

[Deployment preview certificate](../../.github/workflows/deployment-certificate.yml)
uses the official digest-pinned Certbot DNS plugin for `*.preview.aidash.run`.
Configure `GCP_TLS_RENEWAL_WORKLOAD_IDENTITY_PROVIDER`,
`GCP_TLS_RENEWAL_SERVICE_ACCOUNT`, `AIDASH_ACME_EMAIL` and the protected
`certificate-renewal:CLOUDFLARE_CERTIFICATE_DNS_TOKEN` through setup automation.
After privileged code/account inputs are installed, explicitly enable
`GCP_CERTIFICATE_RENEWAL_ENABLED` and dispatch the first issuance on the execution
branch. The daily schedule renews certificates within 30 days of expiry. Verify
the first successful certificate version before enabling random preview deploys.
For this initial certificate-only Terraform activation, explicitly set
`enablement.notifications=false` while mail setup is pending. After configuring
mail, set `enablement.notifications=true` and retain the other selected flags.

The trusted controller validates the wildcard SAN, validity and matching key,
then mounts the PEM only in the edge. Normal reconcile also refreshes quiet
running previews and waits for the changed edge rollout/public health, without
redeploying app code or reissuing outcome mail. Unchanged certificates do not
restart the edge. Random hosts disable per-host ACME issuance, so concrete
hostnames are not disclosed through certificate transparency logs.

Random hosts require managed GCIP tenant bindings and `auth_helper: firebase`.
Configure OAuth callbacks on the fixed `<project>.firebaseapp.com` helper,
not wildcard preview callbacks. The lifecycle controller manages concrete
preview hosts in GCIP `authorizedDomains`, preserving unrelated entries.
Whoever runs the Deployment must coordinate writes to that field with the lifecycle lock because
the API has no etag. Keep host-only `__Host-` session cookies, exact Origin/CSRF
checks and explicit member/workspace authorization; the random URL does not grant
access. Never widen cookie Domain or authenticated CORS to the parent domain.

The trusted deployment identity remains the bootstrap trust root. This change
does not close Issues [#151](https://github.com/kent8192/aidash/issues/151) or
[#152](https://github.com/kent8192/aidash/issues/152); further trust-root hardening
and live IAM denial evidence remain separate gates. Source builds, PR runtime
server/worker identities and the notifier receive neither wildcard key material
nor grants to read that TLS secret.

## Deployment mail

All notification addresses are fixed in trusted code:

| Deployment                         | Recipient                    |
| ---------------------------------- | ---------------------------- |
| `develop` staging                  | `stg.deployment@aidash.run`  |
| Production                         | `prod.deployment@aidash.run` |
| PR environment and standalone test | `test.deployment@aidash.run` |

The sender is `deployment.noreply@aidash.run`. For the current rollout,
`aidash.run` is already onboarded in Cloudflare Email Sending and its DNS
records show **Configured**. Do not onboard a new domain. Review its existing
SPF/DKIM/DMARC and bounce-domain records and confirm account sending availability.
For a future unconfigured account, onboarding is under
**Compute > Email Service > Email Sending > Onboard Domain**. Email Sending
is a beta service; account eligibility and limits must be checked before
activation. See [Cloudflare onboarding](https://developers.cloudflare.com/email-service/get-started/send-emails/)
and [service availability](https://developers.cloudflare.com/email-service/).

Create an account-scoped token with **Email Sending: Edit** and store it only as
`CLOUDFLARE_EMAIL_SENDING_TOKEN` in `deployment-notifications`; set
`CLOUDFLARE_ACCOUNT_ID` there as a variable. These account/token inputs are supplied later in private tfvars as requested.
[Cloudflare setup](../../infra/delivery/cloudflare/README.md) documents the exact
issuance permissions and adopts the existing configured registration. Keep the
DNS token separate.
Configure incoming Email Routing for the three aliases and verify the owner's
private forwarding destination in Cloudflare. The forwarding destination never
belongs in the repository, deployment config, receipts or public logs. Validate
sender authentication on the forwarding path; an alias alone does not prove
that a message came from the deployment pipeline.

The deployment controller writes immutable outcome receipts to a dedicated
private bucket. **Deployment notifications** reads those receipts with a
separate identity and writes only delivery acknowledgements. It sends the URL
only after readiness verification and includes SHA, environment, outcome and
Actions link; it never includes credentials or raw deployment logs. Sanitized
release artifacts and PR bodies omit preview URLs, and public workflow output
contains only safe identifiers and notification counts.

A provider response marked `accepted` means queued, not delivered to the
owner's inbox. Even a provider `delivered` result concerns the alias recipient;
verify the forwarding destination and spam folder separately. Durable leases
and acknowledgements suppress concurrent/acknowledged sends. A timeout or crash
before acknowledgement can still cause a duplicate because the sending API
has no documented idempotency key. Keep the event ID for reconciliation.

Mail transport retries and receipt publication recovery are separate:

| Failure                                                                 | Recovery                                                                                                                                                                                             |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Transient send failure after a receipt exists                           | **Deployment notifications** retries with backoff; never redeploy the application.                                                                                                                   |
| Permanent send rejection                                                | Repair the protected configuration, then dispatch **Deployment notifications** on `automation_branch` with `retry_blocked=true`.                                                                     |
| Ready production, but receipt/outbox publication or its workflow failed | Inspect state, then dispatch `gcp-production.yml` on `main` with the current `main_sha` and `retry_failed=false` to flush the ready outbox without redeploying.                                      |
| Actual failed production deployment                                     | Inspect database, migrations and execution first; explicitly dispatch on `main` with the current `main_sha` and `retry_failed=true`. A refreshed staging receipt cannot authorize an implicit retry. |

Staging receipt recovery rechecks source/readiness and publishes the missing
evidence without restarting the running application. Inspect protected state,
delivery acknowledgements and provider mail logs; do not copy random URLs or
private data into public comments.

## Activation and recovery evidence

During supervised nonproduction acceptance, prove WIF rejects another branch/workflow,
unauthorized comments and stale/fork SHAs; prove exact-source staging readiness,
unknown Host rejection, HTTPS, GCIP sign-in/authorization, CSRF and cookie
boundaries, hostname stability/rotation, and one message through each forwarding
alias. Before production activation, repeat those relevant checks in its
isolated project and demonstrate current main CI, human release provenance,
main/source tree equality and unchanged promoted image digests.

Plan a maintenance window. Deployment closes admission, stops server/worker
writers and verifies active work before applying migrations and restarting.
Unknown or active execution must defer the update; production has no force,
Spot, stop or destroy path. Failed updates remain sealed until someone
reconciles them using the explicit recovery path above. A receipt-storage
failure after readiness does not mean the deployment or migrations failed.
Restore or roll back only after database, Runner and migration compatibility
have been established. An accepted update waiting for active work can resume
through the five-minute schedule after work ends only with valid pending
progress evidence, preserving its accepted generation and bundle.

The production control plane is regional, and production keeps every retained
RWO disk on a regional pd-balanced disk replicated synchronously between
`us-central1-a` and `us-central1-b` (ADR 0067). Its node pools run in
`us-central1-a`; standby pools in `us-central1-b` stay at zero nodes. The server,
worker, Runner, edge, PostgreSQL and NATS are StatefulSets under GKE's Stateful
HA add-on: 20 seconds after a Cluster Node becomes unreachable, the Pod is
force-deleted and its disks are force-attached on a node the standby pool adds
in the other zone. Recovery takes that delay, a new node, the gVisor install
from the bundled image (#394) and Pod start, and standby capacity during a real
zone outage is not guaranteed. The standby pools grow only while the cluster
autoscaler runs, and it halts when more than three nodes and more than 45% of
them are NotReady, so Terraform keeps production at one system node and at most
two Environment nodes. If no standby node has joined ten minutes after the zone
is lost, add one to each standby pool by hand. List the pools with
`gcloud container node-pools list --cluster aidash --location us-central1`. An
autoscaled pool cannot be resized, so for each standby pool run
`gcloud container clusters update aidash --location us-central1 --node-pool <pool> --no-enable-autoscaling`
and then
`gcloud container clusters resize aidash --location us-central1 --node-pool <pool> --num-nodes 1`.
After the first zone is back, the next Terraform apply restores the pools'
autoscaling.
Operations in flight surface as uncertain and are
not replayed. This is not zero downtime, and the loss of the whole region is
accepted; backups stay in `us-central1`. Terraform validates isolation and prevents accidental shared
state use; it does not perform a verified backup restore. Complete the isolated
restore drill and approve RPO/RTO, backup retention and monitoring in
[nonfunctional release requirements](nonfunctional-release.md). Preserve the
independent Home recovery ledger; never rewind it with a database restore.

Useful local verification commands, to run against the exact reviewed checkout:

```sh
python3 -m unittest discover -s infra/gcp/tests -v
python3 -m unittest discover -s infra/gcp/bootstrap/tests -v
bash infra/gcp/check.sh
actionlint .github/workflows/release-pr.yml .github/workflows/gcp-environments.yml .github/workflows/gcp-production.yml .github/workflows/deployment-notifications.yml
git diff --check
```

Record actual command results, tested Git SHA, manifests/digests, the person running it and
private evidence location. These commands are instructions, not a claim that
tests, hosted CI, infrastructure, mail forwarding or restore drills have passed.

## Pre-update Backup

After the seal and before Terraform or Helm changes anything, a production
update takes a Pre-update Backup under the plan `aidash-production-pre-update`
(ADR 0082). The backup is named `pre-update-g<generation>-<sha>`. It holds every
resource in `aidash-production`, with volume data and Secrets, except the Home
recovery ledger claim. If the backup fails, or takes longer than 30 minutes or
the time the deploy still needs, the update fails before it changes anything and
unseals. Read its diagnostic, repair the cause, then dispatch on `main` with
`retry_failed=true`. A retry is a new generation and takes a new backup.
Production refuses to update without this plan. Apply it in step 7 of the
activation checklist before the second production update.

The update entry in the lifecycle state records `pre_update_backup` (the backup's
full name). It records `writers_started` before any writer starts again: the new
release's writers, or the old release's writers when a failure unseals. The
deployment receipt carries both. Both backup plans back up the ProtectedApplication
`aidash`, which the next production deploy after step 7 applies. Until then the
daily plan has nothing to back up and the inspection refuses.

Restore a Pre-update Backup only when all of these hold:

- the update failed, and its entry names `pre_update_backup`;
- the entry has no `writers_started`, so nothing was written after the backup;
- the inspection shows that a forward fix cannot repair the failure.

If `writers_started` is set, a restore loses writes. Fix forward, or treat the
daily backup as the last resort below.

Production deploys only the current protected `main` SHA, so a restore goes
with a release that reverts the failed one. Keep production sealed and prepare
that revert release through the normal release PR. Then run the in-place
restore plan:

```sh
gcloud beta container backup-restore restores create "restore-g<generation>" \
  --project <production-project> --location us-central1 \
  --restore-plan aidash-production-pre-update-in-place \
  --backup "<pre_update_backup>" --wait-for-completion
```

The restore plan selects the application `aidash` and excludes cluster resources,
so `DELETE_AND_RESTORE` replaces only what that application selects. The ledger
claim stays in place with its current contents. Then dispatch `gcp-production.yml` on `main` with the
revert release's `main_sha` and `retry_failed=true`. That update takes its own
Pre-update Backup of the restored state before it deploys. Confirm health and
admission as for a normal update.

## Daily backup: last resort

A daily backup is up to 24 hours old. Restoring it rewinds PostgreSQL, NATS,
the Runner journal and the capability objects, while the ledger stays current.
Authority, Run control and evidence continuity across such a rewind are not
designed yet (#433). Use it only when no Pre-update Backup applies and the data
is otherwise lost, and record what the rewind removed.
