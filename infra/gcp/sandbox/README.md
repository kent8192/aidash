# Disposable credential-boundary sandbox

This tool prepares and operates **two new disposable projects**, one app project
and one BYOK project. It does not import existing projects, run Terraform apply,
change production files, select a gcloud default, or perform provider OAuth.
All live acceptance is pending until real observations are attached to #135/#152.
Local tests only exercise lifecycle and response handling.

Use Python 3.12+, Terraform 1.14.8 and gcloud. The operator needs a concrete
folder/organization, billing account, project-creation quota, project shutdown
and billing permissions, plus the bootstrap roles described in the parent
README. The broker image must come from a reviewed source SHA; record the build
provenance and reviewed digest together before preparing. No approved digest is
currently supplied by the production lock. Never copy sandbox inputs to the
production checkout or change its null lock to make this procedure run.

## Prepare and provision

Set these non-secret values explicitly. `RUN` must be a new directory below
`private/`. Never use an existing production project or state bucket.

```sh
RUN="$PWD/private/credential-sandbox-20261023"
SOURCE_SHA='<reviewed full 40-character SHA>'
BROKER_DIGEST='sha256:<reviewed 64-character digest>'
APP_PROJECT='aidash-sbx-<unique-app-id>'
BYOK_PROJECT='aidash-sbx-<unique-byok-id>'
BILLING_ACCOUNT='<six>-<six>-<six>'
PARENT='folders/<sandbox-folder-number>'
python3 infra/gcp/sandbox/runner.py prepare --directory "$RUN" \
  --source-sha "$SOURCE_SHA" --broker-digest "$BROKER_DIGEST" \
  --app-project "$APP_PROJECT" --byok-project "$BYOK_PROJECT" \
  --billing-account "$BILLING_ACCOUNT" --parent "$PARENT"
```

`prepare` writes a private manifest without cloud calls. Review its source,
billing scope, parent, two IDs and random ownership label. The following is a
**separate cloud-write step**, to execute only after that scope is authorized:

```sh
python3 infra/gcp/sandbox/runner.py create --manifest "$RUN/manifest.json"
python3 infra/gcp/sandbox/runner.py plan --manifest "$RUN/manifest.json" \
  --alert-email '<approved-test-sink@example.com>'
```

`create` performs atomic project creation with the run label, verifies project
number/parent/label and billing association, and persists progress before each
write. A pre-existing ID is never adopted. An interrupted creation resumes only
when that exact run label is present; otherwise resolve the ambiguous attempt
with the operator and choose fresh IDs. Do not manually mark a project owned.
The manifest is a local ownership journal, not a signature or security boundary
against an operator who can edit local files; protect it with the run directory.

`plan` is local-only: it archives `infra/gcp` from the recorded Git SHA into
`$RUN/source`, changes the broker lock **in that isolated copy only**, and writes
Terraform inputs and `plan-commands.json`. It provisions one running unpublished
`test` Environment with no DNS records, `sandbox.invalid` as a non-routable domain,
two GCIP pools, and the real bootstrap/environment modules. The source checkout,
WIF anchors, IAM protections and every `prevent_destroy` remain unchanged.
The sandbox lock is exactly the reviewed digest in the manifest; this is not a
production bypass and must never be passed through the production controller.

Inspect the command array and inputs before executing each command directly:

1. Initialize and plan the copied bootstrap root (local state). Review its saved
   plan; an authorized operator applies `bootstrap/sandbox.tfplan` separately.
2. Build/publish the reviewed broker image to the **new app project's** registry
   and verify its manifest digest equals `BROKER_DIGEST`. Obtain any required
   Cloudflare provider authentication without granting or performing DNS writes.
3. Initialize the copied environments root against the **new app project's**
   state bucket and sandbox run prefix, then review its saved plan. An authorized
   operator applies `environments/sandbox.tfplan` separately. Preserve the local
   bootstrap state until project shutdown is verified.
4. Read generated Terraform outputs privately. Install the reviewed app/Runner
   workload and NetworkPolicies in this sandbox only before Pod probes. Terraform
   creates infrastructure, not the Helm release, GCIP tenant IAM reconciliation,
   dummy users, dummy secrets, or operator impersonation grants.
5. As sandbox administrator, create exactly `aidash-test-cred-dummy` and
   `aidash-other-cred-dummy` in BYOK, each version 1 containing the literal
   `aidash-sandbox-dummy` (no newline). Use stdin/file input for values. Create
   the unprivileged `runtime` and `control` fixture service accounts in app;
   `server`/`worker` come from `environments` outputs. Grant the operator
   permission to impersonate these test principals; **do not grant deploy broker
   impersonation**. Reconcile the own-tenant viewer policy for the server only,
   then create a dummy user in each tenant as administrator.

No live provisioning was performed in this implementation. Project/billing
scope, permissions, reviewed image build, fixture provisioning, app/Helm install,
GCIP tenant grants and alert sink delivery remain execution prerequisites.

## Independent probes and evidence

Create a private `resources.json` with `server`, `worker`, `runtime`, `control`
(service-account emails from these projects), `own_tenant`, `other_tenant` and
`dummy_user`. To prepare assumption 1, also include a **fresh** BYOK IAM policy
with `etag` under `byok_policy`, fetched as the sandbox administrator with
`gcloud projects get-iam-policy "$BYOK_PROJECT" --project="$BYOK_PROJECT" --format=json`.
Keep IAM policy files private and do not print them in agent logs.

```sh
python3 infra/gcp/sandbox/recipes.py --manifest "$RUN/manifest.json" \
  --resources "$RUN/resources.json" --output "$RUN/requests"
python3 infra/gcp/sandbox/runner.py probe --manifest "$RUN/manifest.json" \
  --requests "$RUN/requests/prefix-server-own.json"
```

Review and execute **one request file at a time**, positive control before its
negative counterpart. Refresh policies and regenerate requests between IAM
mutations: stale etags are conflicts, never IAM-denial evidence. Restore the
baseline after every policy-changing control, especially audit configuration.
Run destructive broker lifecycle probes last. If a deny probe succeeds, stop
acceptance, repair the sandbox boundary, restore the affected identity/policy,
and re-probe. Do not continue as if that assumption passed.

The runner obtains impersonation tokens internally, sends them only to fixed
Google API hosts without redirects, checks all project paths before obtaining a
token and stores no response body. `PERMISSION_DENIED` qualifies only with exact
Google `ErrorInfo` domain, reason, permission and resource. A missing structured
resource, denied impersonation, API-disabled response, wrong principal boundary,
404, timeout or HTML ingress rejection is **inconclusive**. Correlate the request
with audit evidence if Google omits details; do not weaken the classifier to
accept arbitrary 403s. `observation_only` means the API call succeeded but its
semantic contract still needs the independent check below. A failed operation
stops the batch; any failed, inconclusive or observation-only result returns a
nonzero exit status after saving its evidence.

`probe-results.json` contains only probe ID, assumption, expected outcome,
identity, project, HTTP status, classification, SHA and timestamps. It does not
contain request/response payloads, tokens, cookies, URLs or exception text. The journal retains up to 1,000 attempts across invocations; archive it privately
before starting another run. Do not infer coverage from the count. Keep a sanitized matrix with a row for
**every** control and negative case, its timestamp and audit/run evidence ID.
No assumption is completed merely because one request reports `passed`.

| Assumption | Prepared requests                                                       | Additional independent live acceptance                                                                                                                                                                                                                        |
| ---------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1          | `grant-create`, `grant-manage`, `grant-payload`, `audit-config-removal` | Fresh etag each time; reread resulting policy as admin. Fixed grants succeed; payload/other-role grants and audit-only change fail at `projects.setIamPolicy`. Restore policy/audit baseline between calls.                                                   |
| 2          | `prefix-server-own`, `prefix-server-other`                              | Confirm project-number resource names, own metadata success, foreign prefix denial. Exercise add/disable/destroy management using disposable versions and verify state transitions; retain a control secret until all read probes finish.                     |
| 3          | `payload-{broker,server,worker,runtime,deploy}-{own,other}`             | Own broker gets the exact dummy bytes; every other identity and cross-prefix broker fails. One configured broker Environment is tested; no multi-environment result is inferred.                                                                              |
| 4          | Pod and internet procedures below                                       | Same broker URL/request from current server/worker Pod succeeds, internet ingress fails; an allowed metadata client works and each unprivileged app/Runner/execution/installer Pod fails metadata access. Authentication denial alone does not prove ingress. |
| 5          | `kms-sign-worker`, `kms-sign-server`                                    | Export the public key as admin, sign the fixed dummy bytes as worker, verify Ed25519 signature locally, and record only verification boolean/key version. The generic signing request remains observation-only; it intentionally discards the signature.      |
| 6          | `gcip-lookup-own`, `gcip-lookup-other`, `gcip-lookup-project`           | Own lookup returns exactly the seeded user; other tenant and project-root lookups fail at `firebaseauth.users.get`. Verify own and foreign users exist first using admin. Generic own lookup remains observation-only.                                        |
| 7          | Interactive fixture/session procedure below                             | Record `validSince` before/after each account operation and verify old session behavior independently. No canned fixture result counts.                                                                                                                       |
| 8          | `deploy-deny-*` for all ten permissions                                 | Tag binding exists; untagged control operations succeed, tagged broker operations fail; deploy cannot detach tag; authorized broker deploy works; deletion/restore cannot remove protection. WIF and alert acceptance below remain independent.               |
| 9          | None                                                                    | Hosted Plan-secret refresh remains deferred to #248.                                                                                                                                                                                                          |

### Pod ingress and metadata

Use an isolated kubeconfig under `$RUN` obtained with explicit project:

```sh
gcloud container clusters get-credentials aidash --zone=us-central1-a \
  --project="$APP_PROJECT" --dns-endpoint
```

Set `KUBECONFIG="$RUN/kubeconfig"` **before** that command. Confirm its cluster
resource is in `APP_PROJECT` before any `kubectl --context=<sandbox-context>`
operation. In `aidash-test`, run the same non-secret broker health/request path
from the reviewed server and worker Pods; use `curl --silent --output /dev/null
--write-out '%{http_code}' --max-time 15 <broker-url>/<reviewed-health-path>`.
Repeat from an internet machine. Record Pod UID, image digest, network-policy
revision, origin and HTTP status only. A healthy Pod positive control must
succeed before external failure counts. For a signed broker request keep the
capability in a private file and do not print it or put it in process arguments.

From each real workload Pod run a bounded request to
`http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/email`
with `Metadata-Flavor: Google`; discard the response body, record exit/status.
The allowed server/worker control establishes metadata availability. The Runner,
unprivileged app workloads, execution sandbox, guard and installer must match
the deployed metadata-policy contract. Do not assert that a token endpoint's
HTTP 200 grants useful cloud permission; test principal authorization separately.

### KMS signing verification

Run as the authorized sandbox worker, with explicit app project, using
`gcloud kms asymmetric-sign --location=us-central1 --keyring=aidash-test-capability
--key=capability --version=1 --input-file=<dummy-bytes> --signature-file=<private-signature>
--project="$APP_PROJECT" --impersonate-service-account=<worker-email>`.
This omits `--digest-algorithm` to sign raw Ed25519 input, as documented in
[Cloud KMS signing](https://docs.cloud.google.com/kms/docs/create-validate-signatures).
Export the public key with `gcloud kms keys versions get-public-key 1` and the
same location/keyring/key/project flags, then run
`openssl pkeyutl -verify -pubin -inkey <public.pem> -rawin -in <dummy-bytes>
-sigfile <private-signature>`. Record the exit code, public key version and source
SHA; retain neither signing tokens nor raw API responses. The unauthorized
server request must fail at `cloudkms.cryptoKeyVersions.useToSign`.

### GCIP validSince and application sessions

Use only dummy sandbox accounts and private interactive application sessions.
For each of password change, password reset completion, admin refresh-token
revocation, disabling and deleting the account: establish a fresh session,
privately read `validSince`, perform the one operation, reread it when the user
still exists, and use the old session against an authenticated Aidash endpoint.
Record operation name, timestamp-change boolean, session allowed/denied boolean,
HTTP status and bounded propagation interval. Deleted/disabled users must be
denied regardless of timestamp behavior. Do not assume every password action
advances the timestamp; any contrary observation blocks the corresponding
security assumption and must be repaired or explicitly deferred. Redact user
identifiers, callback URLs, cookies and token bodies from artifacts.

### Deploy tag, WIF and alerts

For the ten generated deny requests, audit the **denied permission itself**;
`undelete` on a live account may return a lifecycle error and therefore cannot
prove deny. Run delete/undelete/tag-lifecycle controls with a disposable fixture
account under administrator supervision after the primary broker probes. Use
an untagged control account with an explicit allow grant to prove that the
operation is otherwise reachable; remove that temporary grant afterwards.
Capture the broker tag-binding resource and policy version privately. The deploy
identity's attempt to delete that exact tag binding must fail. Administrator
remove/rebind controls require their own review because they temporarily remove
the sandbox broker deny target. Verify deny and `actAs` after restoration.

Preserve ADR 0081: `gcp-environments.yml` on protected `develop/0.1.0` with
Environment `nonproduction`, and `gcp-production.yml` on `main` with Environment
`production`. Never expand WIF workflow/ref claims to a sandbox feature branch.
The production identity is not provisioned or exercised by this two-project
nonproduction envelope. Reuse an existing protected successful auth run only as
positive authentication evidence for its exact SHA/identity/claims; it does not
prove any negative case or sandbox IAM outcome.

Protected-ref-only negative probe jobs must be reviewed in this same
implementation PR and run **after its normal merge**. Their integration into
the shared workflow requires the coordinating agent; this directory does not
silently modify a shared workflow. Required cases are feature ref, missing
Environment, wrong Environment/workflow, and cross-tier identity. The positive
control uses the exact protected workflow plus Environment and must obtain a
short-lived identity; each negative must fail at WIF/STS, not at workflow syntax,
missing variables, action startup or an unrelated API. Record only `environment`,
`ref`, `workflow_ref`, source/merge SHA, identity, run URL and error class. Never
log a JWT or access token. A production reviewer wait is observed through GitHub
run state, without dispatching an actual production deployment for this sandbox.

Trigger each alert separately in the sandbox: broker revision change; broker
service/account IAM change; non-broker `AccessSecretVersion`; BYOK audit-config
change; and broker-account use by Cloud Run service/job, Compute instance and
GKE node pool. Check the audit method/resource/request shape matches the real
policy and the test sink receives the corresponding event. A broker's authorized
read is the negative alert control. Record event timestamp/insert ID, alert
policy ID and sink receipt timestamp only. Restore audit configuration promptly.
No delivery receipt means pending, even if Cloud Logging contains the event.

## Cleanup

After probes, revoke dummy provider grants in their owner UI if any were used,
disconnect Plan Connections/Provider Credentials, delete private callback/token
artifacts, and record the boolean outcome. Dummy-secret IAM probes require no
real provider credentials. The envelope does not revoke the operator's personal
ADC/gcloud login. Delete only run-specific credential files; preserve evidence
and state until shutdown verification succeeds.

```sh
python3 infra/gcp/sandbox/runner.py cleanup --manifest "$RUN/manifest.json"
```

Cleanup checks ID, project number, run label and parent, unlinks billing, requests
project shutdown, and reads back `DELETE_REQUESTED` plus billing disabled and no
billing account. It attempts both projects after a partial failure and resumes
safely. Ownership mismatches never trigger deletion. A missing/inaccessible
project is not treated as successful cleanup. Re-run after repairing access;
retain `cleanup_pending` until every created project is observed shut down.
Do not strip `prevent_destroy` or run Terraform destroy on protected resources.
Google's recovery period remains pending; this is **verified shutdown**, not
immediate permanent deletion. See [Google's project lifecycle documentation](https://docs.cloud.google.com/resource-manager/docs/delete-restore-projects).

## Real provider acceptance (separate from #152)

After app/desktop changes are available, the owner uses Provider Authorization
for OpenRouter and the personal ChatGPT Plan Connection UI with interactive
consent. Do not paste provider keys or callback URLs into the agent. Credentials
stay in the configured Store. Execute one model call, a multi-step tool call,
refresh/restart recovery and disconnect; verify the correct Subject owns the
connection, disabled use still permits status/disconnect, and later calls fail
after disconnect without fallback to Provider Credentials. Record only provider,
client version/source SHA, case ID, status, timestamps and sanitized error class.
Desktop/browser consent and real provider calls remain owner-authorized live
acceptance; fake providers and local unit tests cannot complete this checklist.
