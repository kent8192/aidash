# Release deployment operations

Keep deployment and notification enablement disabled until account setup is
complete. Enable nonproduction for a supervised acceptance window; keep
production disabled until its isolated readiness gates pass. Local tests establish controller,
workflow and Terraform contracts; they do not establish running infrastructure,
mail delivery, recovery objectives or hosted CI success.

## Activation checklist

Complete these steps in order. The Cloudflare sending domain already reports
**Configured**; its API credential and incoming forwarding aliases still need
operator setup and delivery verification.

| Step | Required work                                                                                                                                                                                                                                                                                                                                                                          | Completion evidence                                                                                                                                    |
| ---- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1    | Keep all three enablement variables false. Publish the reviewed controller/workflows to protected `main` and default `develop/0.1.0`; synchronize main back into develop before the first release candidate.                                                                                                                                                                           | Both protected branches contain the implementation and the candidate includes current main history.                                                    |
| 2    | Protect `main` and `develop/0.1.0`; create the four GitHub Environments with the branch restrictions below. Require production reviewers and prevent self approval. Create/install the repository-scoped release App, configure its client ID/private key and matching bot login, and require CI plus the App-owned **Staging deployment** check.                                      | Reviewed branch rules, required check integration and Environment protection rules.                                                                    |
| 3    | Provision separate nonproduction/production GCP bootstrap stacks with billing, APIs, private remote state/release/notification buckets, runtime identities and exact WIF restrictions. Configure the production staging-evidence/image read grants. Fill both example JSON configs and install the listed GitHub variables and Environment runtime secrets.                            | Reviewed Terraform plans, isolated bootstrap outputs and denied cross-tier/unauthorized WIF requests.                                                  |
| 4    | Configure the existing Cloudflare DNS zone token. Issue `*.preview.aidash.run` with DNS-01, put the PEM certificate/key JSON in the configured nonproduction Secret Manager secret and arrange renewal. Configure GCIP tenants and fixed Firebase-helper OAuth/SSO callbacks, with separate production clients.                                                                        | Valid wildcard certificate/key, exact callback registrations and successful authenticated access.                                                      |
| 5    | In `deployment-notifications`, set `CLOUDFLARE_ACCOUNT_ID` and an **Email Sending: Edit** token as `CLOUDFLARE_EMAIL_SENDING_TOKEN`. Configure Email Routing for the three stg/prod/test aliases and verify their private forwarding destination.                                                                                                                                      | Existing SPF/DKIM/DMARC reviewed, sender accepted and each alias reaches the intended inbox.                                                           |
| 6    | During a supervised nonproduction window, enable `GCP_ENVIRONMENTS_ENABLED`, `GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED` and config `release_flow_enabled`. Trigger official CI for the current develop head and an authorized `/preview up`; verify the staging/release gate, random hosts, sign-in, Host/CSRF boundaries, fork rejection, stop/resume/retirement and mail retry behavior. | Exact source/image proof, ready App check, stable/rotated host behavior and actual stg/test notifications; no real secrets or URLs in public evidence. |
| 7    | Complete production IAM denial evidence, backup/isolated restore drill, RPO/RTO and retention decisions, monitoring, maintenance and migration recovery procedures.                                                                                                                                                                                                                    | Approved live readiness evidence, including the single-zone data/availability limits below.                                                            |
| 8    | Enable `GCP_PRODUCTION_ENABLED` for the reviewed production rollout. Human-merge the validated release PR, approve the production Environment and verify main CI, original digest promotion, HTTPS/sign-in and the production notification/forwarding path.                                                                                                                            | Running production source/digests and actual production mail arrival; retain evidence privately.                                                       |

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
SHA **T** separately. T must be an approved ancestor of protected `main`; the
bundle includes app, postgres, sandbox, observer, nats, control, edge and caddy.
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

## Account and GitHub setup

Publish the reviewed workflow and controller changes to the appropriate
protected branches before enabling them. `automation_branch` is the protected
repository default branch, currently `develop/0.1.0`, on which nonproduction
workflow-run, comment and scheduled automation executes. `trusted_branch` is
`main`: privileged jobs pin one reviewed main commit and never execute PR code.
Run manual **GCP environments** operations on `automation_branch`, not on the
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

Use repository settings or file/stdin input to configure secret values. Do not
place them in shell arguments, `.tfvars`, Git history, workflow artifacts or chat.
Leave `GCP_ENVIRONMENTS_ENABLED`, `GCP_PRODUCTION_ENABLED` and
`GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED` false during account setup, then enable
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

Prepare a wildcard certificate for `*.preview.aidash.run` through a trusted
DNS-01 process. Manually provision Secret Manager metadata and a version in the
nonproduction application project, named by config `preview_tls_secret`
(for example, `aidash-preview-wildcard-tls`). The version is JSON containing
`certificate_pem` (full PEM chain) and `private_key_pem`; its values never enter
Terraform state. The trusted deployment identity reads this secret through its
existing Secret Manager permissions. No bootstrap resource or new accessor
variable is required. The controller checks certificate validity,
the exact wildcard SAN and matching private key, then mounts it only in the
trusted edge. Renew it by publishing a new secret version. Random hosts disable
per-host ACME issuance, which would disclose concrete hostnames through
certificate transparency logs.

Random hosts require managed GCIP tenant bindings and `auth_helper: firebase`.
Configure OAuth callbacks on the fixed `<project>.firebaseapp.com` helper,
not wildcard preview callbacks. The lifecycle controller manages concrete
preview hosts in GCIP `authorizedDomains`, preserving unrelated entries.
Operators must coordinate writes to that field with the lifecycle lock because
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
`CLOUDFLARE_ACCOUNT_ID` there as a variable. These account/token inputs still
need to be supplied for activation; no real sending credentials were added to
this worktree. Keep the DNS token separate.
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
Spot, stop or destroy path. Failed updates remain sealed until an operator
reconciles them using the explicit recovery path above. A receipt-storage
failure after readiness does not mean the deployment or migrations failed.
Restore or roll back only after database, Runner and migration compatibility
have been established. An accepted update waiting for active work can resume
through the five-minute schedule after work ends only with valid pending
progress evidence, preserving its accepted generation and bundle.

The production control plane is regional, but node pools and retained RWO data
are pinned to one zone. This does not provide application/data zonal failover
or zero downtime. Terraform validates isolation and prevents accidental shared
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

Record actual command results, tested Git SHA, manifests/digests, operator and
private evidence location. These commands are instructions, not a claim that
tests, hosted CI, infrastructure, mail forwarding or restore drills have passed.
