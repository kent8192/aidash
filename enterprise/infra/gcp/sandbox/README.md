# Disposable credential-boundary sandbox

This tool prepares and operates **two new disposable projects**, one app project
and one BYOK project. It does not import existing projects, run Terraform apply,
change production files, select a gcloud default, or perform provider OAuth.
All live acceptance is pending until real observations are attached to #135/#152.
Local tests only exercise lifecycle and response handling.

Use Python 3.12+, Terraform 1.14.8, OpenSSL 3, kubectl and gcloud. The person running this verification needs a concrete
explicit parent (`none`, folder or organization), billing account, project-creation quota, project shutdown
and billing permissions, plus the bootstrap roles described in the parent
README. The broker image must come from a reviewed source SHA; record the build
provenance and reviewed digest together before preparing the environments root.
Project creation and bootstrap preparation may precede the private image build.
No approved digest is currently supplied by the production lock. Never copy sandbox inputs to the
production checkout or change its null lock to make this procedure run.

## Prepare and provision

Set these non-secret values explicitly. `RUN` must be a new directory below
`private/`. Never use an existing production project or state bucket.

```sh
RUN="$PWD/private/credential-sandbox-20261023"
SOURCE_SHA='<reviewed full 40-character SHA>'
APP_PROJECT='aidash-sbx-<unique-app-id>'
BYOK_PROJECT='aidash-sbx-<unique-byok-id>'
BILLING_ACCOUNT='<six>-<six>-<six>'
PARENT='none' # Explicit personal/no-organization project; or folders/<number>
python3 enterprise/infra/gcp/sandbox/runner.py prepare --directory "$RUN" \
  --source-sha "$SOURCE_SHA" \
  --app-project "$APP_PROJECT" --byok-project "$BYOK_PROJECT" \
  --billing-account "$BILLING_ACCOUNT" --parent "$PARENT"
```

`prepare` writes a private manifest without cloud calls. Omitting `--broker-digest`
records null rather than inventing an image. If a reviewed image already exists,
`--broker-digest "$BROKER_DIGEST"` remains supported and locks that image at
preparation. Review its source, billing scope, parent, two IDs and random
ownership label. The following is a
**separate cloud-write step**, to execute only after that scope is authorized:

```sh
python3 enterprise/infra/gcp/sandbox/runner.py create --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/runner.py plan --phase bootstrap --manifest "$RUN/manifest.json" \
  --alert-email '<approved-test-sink@example.com>'
```

`create` performs atomic project creation with the run label, verifies project
number/parent/label and billing association, and persists progress before each
write. A pre-existing ID is never adopted. An interrupted creation resumes only
when that exact run label is present; otherwise resolve the ambiguous attempt
with the person running that attempt and choose fresh IDs. Do not manually mark a project owned.
The manifest is a local ownership journal, not a signature or security boundary
against someone who can edit local files; protect it with the run directory.

`plan --phase bootstrap` is local-only: it archives `enterprise/infra/gcp` from the recorded
Git SHA into `$RUN/source`, records that SHA and archive hash in
`source-manifest.json`, and writes only bootstrap inputs and
`bootstrap-plan-commands.json`. The archived environment image lock remains
byte-for-byte unchanged; no environment inputs or commands are generated. This
allows the new project's Artifact Registry to exist before a private image build.

1. Review and execute the commands in `bootstrap-plan-commands.json`, then review
   the saved bootstrap plan. An authorized person applies that exact plan
   separately. Keep its local state and saved plan in place.
2. Build the broker privately from the **same recorded SHA** and publish it to
   `us-central1-docker.pkg.dev/$APP_PROJECT/aidash/broker` in the newly owned
   project. Record the actual build provenance and reviewed `sha256:` digest as
   `BROKER_DIGEST`. Bootstrap creates Artifact Registry; using Cloud Build also
   requires separately scoped API enablement and a build service account with
   the necessary sandbox registry/logging access. This tool does not implicitly
   authorize or run a build, select a public service, or supply a placeholder.
3. Register the actual digest once, then prepare environments from the retained
   source archive:

```sh
python3 enterprise/infra/gcp/sandbox/runner.py set-broker-image --manifest "$RUN/manifest.json" \
  --source-sha "$SOURCE_SHA" --digest "$BROKER_DIGEST"
python3 enterprise/infra/gcp/sandbox/runner.py plan --phase environments --manifest "$RUN/manifest.json"
```

`set-broker-image` is local-only, requires the exact manifest/source-receipt SHA,
and refuses to replace an already registered image or change a prepared
environment. It does not verify remote image provenance; whoever runs the build must
review the private build receipt and registry digest before registration.
`plan --phase environments` requires that registered digest and prior bootstrap
preparation. It reuses the same isolated archive and changes the broker lock
**in that isolated copy only**. It writes environment inputs and
`environments-plan-commands.json`; neither bootstrap inputs/state nor its saved
plan is overwritten. Missing digest blocks environment preparation before any
write. Every phase refuses existing generated inputs/commands/saved plans;
resolve an interrupted local preparation explicitly instead of overwriting a
reviewed plan.

Review and execute the environment commands against the new app project's state
bucket and sandbox run prefix, then review/apply the saved environment plan
separately. Obtain any required Cloudflare provider authentication without
performing DNS writes. The inputs create one running unpublished `test`
Environment with no DNS records, `sandbox.invalid`, two GCIP pools and the real
modules. Preserve bootstrap state until project shutdown is verified. Install
the reviewed app/Runner workload and NetworkPolicies before Pod/session probes;
missing actual workloads fail acceptance.

For a digest already known at `prepare`, the original `plan --manifest
"$RUN/manifest.json" --alert-email ...` (default `--phase all`) remains available
and writes `plan-commands.json` for both roots in one operation. Use either that
combined path or the staged path. The source checkout, production null lock,
WIF anchors, IAM protections and every `prevent_destroy` remain unchanged.
Never copy sandbox inputs into the production controller. Digest-free project
cleanup remains available if build/provisioning cannot proceed.

## Executable controls and evidence

`controls.py` runs named phases only when invoked. All phases require both
manifest-owned projects to be active. **Every phase below except `alert-plan`
may read cloud metadata; fixtures, manage, tags, lifecycle, broker-undelete-last,
recover-broker, alert and gcip also write sandbox resources.** Review these commands with the selected manifest
before the separate cloud execution authorization. None was run during this
implementation. All stdout/stderr from child processes is captured, never
forwarded. Private credentials stay in memory or mode-0600 files under `RUN`.

```sh
python3 enterprise/infra/gcp/sandbox/controls.py discover --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/controls.py fixtures --manifest "$RUN/manifest.json" \
  --verification-member 'user:YOUR_VERIFICATION_EMAIL'
python3 enterprise/infra/gcp/sandbox/controls.py identities --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/recipes.py --manifest "$RUN/manifest.json" \
  --resources "$RUN/resources.json" --output "$RUN/requests"
```

`discover` reads the isolated roots' applied `configuration` and `environments`
outputs, the actual `test` GKE node-pool `config.serviceAccount`, and Cloud Run
`aidash-test-credential-broker` status. It writes concrete server, worker, node,
broker, deploy and tenant IDs. The node payload probe therefore exercises the
configured GKE node identity, **not an invented runtime account**. `fixtures`
creates only the unprivileged `aidash-sandbox-control` and
`aidash-sandbox-target` accounts, dummy secrets, two tenant dummy users and the
own-tenant server viewer grant. The exact dummy password is in
`$RUN/dummy-login.json`; never paste it into agent logs. It also grants only the
specified verification member TokenCreator on the test identities so they can be exercised.
`identities` checks each impersonated access token's Google tokeninfo email and
expiry privately before that identity's results can count as evidence.

Fixture setup is intentionally create-only. A partially completed invocation
must be inspected from private state before retry; do not adopt pre-existing
fixtures. Entire-project shutdown remains the final cleanup boundary.

Run each reviewed request independently, positive before negative:

```sh
python3 enterprise/infra/gcp/sandbox/runner.py probe --manifest "$RUN/manifest.json" \
  --requests "$RUN/requests/prefix-server-own.json"
```

Requests use resource-project billing, matching resource APIs without an
`x-goog-user-project` override. No extra Service Usage Consumer grant is added
to make a quota-project denial resemble an IAM decision. Every CLI command
still names its target project explicitly. Google `ErrorInfo` must match the
intended IAM domain, permission, resource and reason; arbitrary 403s, missing
APIs, transport failures and missing impersonation grants stay inconclusive.
Every 2xx forbidden response fails and stops the batch, including empty 204,
non-JSON and list/scalar bodies. Nonpassing results return a nonzero exit status
**after** writing evidence. A call without a semantic assertion stays
`observation_only`; it is not an accepted assumption.

`probe-results.json` records method, normalized target, permission, denial reason,
SHA-256 of the canonical request spec, identity, project, expected/observed status,
source SHA and timestamps. Thus regenerated same-ID policy/tenant requests remain
distinguishable. It stores no payload, token, cookie or response body. Controls
record bounded metadata in `controls-evidence.json`, including the executable
and discovered-resource digests. These are real observations only after actual
execution; unit-test mock outcomes must never be copied into this journal.

### Assumptions 1–3: policy, prefixes and payloads

For assumption 1, fetch a fresh policy as administrator **before each grant**:

```sh
gcloud projects get-iam-policy "$BYOK_PROJECT" --project="$BYOK_PROJECT" \
  --format=json > "$RUN/byok-policy-fresh.json"
python3 - "$RUN" <<'PY_POLICY'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
r = json.loads((p / 'resources.json').read_text())
r['byok_policy'] = json.loads((p / 'byok-policy-fresh.json').read_text())
(p / 'resources.json').write_text(json.dumps(r))
PY_POLICY
python3 enterprise/infra/gcp/sandbox/recipes.py --manifest "$RUN/manifest.json" \
  --resources "$RUN/resources.json" --output "$RUN/requests-policy-1"
```

Execute `grant-create.json`, `grant-manage.json`, `grant-payload.json`, and
`audit-config-removal.json` individually from freshly generated directories.
The first two must succeed and a fresh administrator `get-iam-policy` must show
only the intended role/member change. The latter two must fail at
`resourcemanager.projects.setIamPolicy`; etag conflicts do not qualify. Restore
with `gcloud projects set-iam-policy "$BYOK_PROJECT" <baseline-file>
--project="$BYOK_PROJECT"` after replacing the baseline's etag with the fresh
policy's etag. If audit removal unexpectedly succeeds, restore immediately and
stop acceptance. Do not reuse stale etags or call a 200 observation accepted
without comparing the policy.

Assumption 2 uses `prefix-server-{own,other}.json` and
`foreign-manage-{addversion,delete}.json`, then actual own-prefix lifecycle:

```sh
python3 enterprise/infra/gcp/sandbox/controls.py manage --manifest "$RUN/manifest.json"
```

This adds dummy bytes to `aidash-test-cred-management` as the actual server,
observes `DISABLED` and `DESTROYED` after the corresponding calls, and deletes
the management secret. It never destroys the read controls. Assumption 3 uses
all `payload-{broker,server,worker,node,deploy}-{own,other}.json`: only own broker
must return the exact dummy bytes. Cross-prefix metadata/payload denials need
the project-number resource name, independently of the project-ID URL.

### Assumption 4: actual Pod ingress and metadata

```sh
python3 enterprise/infra/gcp/sandbox/controls.py kubeconfig --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/controls.py pods --manifest "$RUN/manifest.json" \
  --probe-image 'YOUR_REVIEWED_CURL_IMAGE@sha256:YOUR_REVIEWED_DIGEST'
```

The image is an execution prerequisite: a reviewed digest containing `curl`,
runnable as a restricted ephemeral container. `pods` inventories current Running
Pods in `aidash-test`, `aidash-test-sandbox`, `aidash-test-trusted` and
`aidash-test-installer`. Actual server and worker labels, Runner label and all
execution/guard/installer namespaces are required. It attaches ephemeral probe
containers to those **existing Pods**, preserving their network namespaces and
NetworkPolicy selectors; missing image/attach support is not a network denial.
The namespaces, Pod UID and observations are recorded. Keep the real execution
Pod running for this phase (start a bounded sleep task using the normal Runner).

The metadata test requests only
`/computeMetadata/v1/instance/service-accounts/default/email` with the required
header. Expected allowance comes from the applied root's Kubernetes identities:
only `app-aidash-server` and `app-aidash-worker` in `aidash-test`, with the matching
component role and no Runner marker, qualify. Their responses must equal the
discovered Google SA emails. Runner, execution, guard, installer and other
workloads must time out regardless of their metadata label. The probe separately
checks that only server/worker carry `aidash.run/metadata-server: allowed`; any
such label key on a denied workload is a failure even when the network blocks.
A different HTTP rejection is inconclusive. No metadata token is requested or
logged.

There is **no broker health endpoint**. The server/worker Pod probes send an
unsigned GET to the real `/api/v1/chat/completions` path with a random non-secret
correlation marker. Expected app response is HTTP 401 with
`error.code=capability_invalid_signature`. Capability verification precedes route
processing and Secret Store/provider access, so these calls cannot invoke a
provider. The corresponding Cloud Run request log must name the exact ready
revision, marker and 401. Log propagation is not a failure of ingress; collect
again using the marker stored in the private journal:

```sh
python3 enterprise/infra/gcp/sandbox/controls.py request-log --manifest "$RUN/manifest.json" \
  --marker 'THE_24_HEX_MARKER_FROM_CONTROLS_EVIDENCE'
```

Run this phase on the verification workstation **outside the sandbox VPC** for its
internet control. That request must return an ingress 403/404 and must not return
the broker's distinctive capability rejection. No arbitrary HTTP failure counts.
Run Pod controls before revision-alert probes, or rediscover the new ready
revision first. Ephemeral probe containers terminate after at most 15 seconds;
Pod deletion/normal rollout removes their records, and project shutdown removes
them with the cluster.

### Assumptions 5–7: signing, tenant lookup and session invalidation

```sh
python3 enterprise/infra/gcp/sandbox/controls.py kms --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/runner.py probe --manifest "$RUN/manifest.json" \
  --requests "$RUN/requests/kms-sign-server.json"
python3 enterprise/infra/gcp/sandbox/runner.py probe --manifest "$RUN/manifest.json" \
  --requests "$RUN/requests/gcip-lookup-own.json"
```

`kms` signs fixed dummy bytes as the actual worker using the real
`aidash-test-capability/capability/1` Ed25519 key, exports the public key and runs
`openssl pkeyutl -verify -rawin`; it records only verification success and removes
the temporary signature. Raw-data signing follows
[Cloud KMS's signing contract](https://docs.cloud.google.com/kms/docs/create-validate-signatures).
The server negative must fail at `cloudkms.cryptoKeyVersions.useToSign`.
`gcip-lookup-own` asserts the exact seeded local ID; execute the other-tenant and
project-root lookup negatives independently. Administrator fixture setup proves
that both tenants/users exist, so 404 is not accepted as isolation evidence.

For assumption 7 the owner signs in **as the seeded own-tenant dummy user** through
the sandbox app and saves the session cookie privately, after verifying the
sandbox project and user. Use the actual backend service with a local forward:

```sh
kubectl --kubeconfig="$RUN/kubeconfig" \
  --context="gke_${APP_PROJECT}_us-central1-a_aidash" -n aidash-test \
  port-forward service/app-backend 18080:8080
```

The owner creates `$RUN/session.json` (mode 0600) with
`{"origin":"http://127.0.0.1:18080","cookie":"<private Cookie header>"}`.
Do not put that value in commands, screenshots or agent logs. Then run exactly
one operation with that fresh session:

```sh
python3 enterprise/infra/gcp/sandbox/controls.py gcip --manifest "$RUN/manifest.json" \
  --change password --session-file "$RUN/session.json"
```

Repeat separately with `--change reset`, `revoke`, `disable`, and `delete`, obtaining
a fresh owner session first. The executable uses real tenant account APIs,
reads `validSince` before/after, verifies `/auth/session` was authenticated before
mutation, then checks denial with the **old cookie**. Reset obtains and consumes
an OOB code internally without email delivery/logging. Password changes update
`dummy-login.json`; no returned token or callback is printed. Complete any
provider acceptance and grant cleanup that uses this identity before running
disable/delete. Between disable and delete, run `controls.py gcip-reenable
--manifest "$RUN/manifest.json"`, then have the Deployment Administrator restore
the exact Aidash identity through `POST /api/dashboard/identities/{id}/restore`
and obtain a fresh owner session. GCIP re-enablement alone does not restore
Aidash authority; see [Account Status](../../../../docs/authorization.md#dashboard-identity).
To restore the dummy user after delete, use tenant `accounts:batchCreate`
and `accounts:update` via the fixture setup in a new run; do not recreate a
production user. A still-valid session is a failure to investigate (including
propagation/cache interval), not accepted by waiting indefinitely or assuming
all operations advance `validSince`.

### Assumption 8: deny/tag/lifecycle and alerts

Deny-policy tests must distinguish an explicit deny from simply missing an allow.
The following **optional, separately reviewed sandbox-only fixture phase** saves
exact broker/control policies, temporarily grants deploy TokenCreator/KeyAdmin
on the tagged broker and untagged control, and grants broker/control delegation
to the separate target account. It never changes production or removes the
broker's tag. If the deny is broken this substrate could expose dummy credentials;
therefore run one case at a time, stop on unexpected allow, and restore promptly.

```sh
python3 enterprise/infra/gcp/sandbox/controls.py deny-fixtures --manifest "$RUN/manifest.json"
# Execute untagged-* positive files, then deploy-deny-* negatives individually.
python3 enterprise/infra/gcp/sandbox/controls.py tags --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/controls.py lifecycle --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/controls.py restore-deny --manifest "$RUN/manifest.json"
```

The generated requests cover nine denied permissions; the tenth, undelete, uses
the explicit final lifecycle phase below. `tags` verifies the
same control-account policy write before tagging, while tagged, and after
restoration, and tests deploy detachment of the real broker tag binding. A
`finally` block restores the fixture tag. `lifecycle` observes control disable,
restore, delete and same-numeric-ID undelete, then verifies real broker deletion
is denied and `actAs` remains. It persists the control UID before deletion for
recovery with the commands below.
A direct undelete on a live broker can fail before IAM evaluation and is not
provided as a generated probe. Deletion denial proves only the delete permission;
it cannot substitute for undelete evidence. `restore-deny` restores baseline IAM
with fresh etags, verifies exact original bindings, and revokes newly generated
**broker and control** user-managed keys. Both key inventories are completed
before any fixture grant; restore verifies both inventories equal their original
sets before claiming completion. Existing keys are preserved, and private-key
payloads are never recorded. An interrupted/failed restore retains its baseline
journal; rerun `restore-deny` before proceeding.

Control disable, delete and tag creation persist recovery identifiers before
mutation. If interrupted, inspect the corresponding private journal and observed
resource state before retrying. These commands use the saved exact identity/UID:

```sh
CONTROL_SA=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["control"])' "$RUN/resources.json")
CONTROL_UID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["unique_id"])' "$RUN/control-deleted.json")
# After observing that the saved control UID is deleted:
gcloud iam service-accounts undelete "$CONTROL_UID" --project="$APP_PROJECT"
# After observing that the saved control account is disabled:
gcloud iam service-accounts enable "$CONTROL_SA" --project="$APP_PROJECT"
```

For interrupted tag creation/removal, `control-tag-recovery.json` contains the
exact parent and tag value. After observing that this control binding exists:

```sh
CONTROL_TAG_PARENT=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["parent"])' "$RUN/control-tag-recovery.json")
CONTROL_TAG_VALUE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tag_value"])' "$RUN/control-tag-recovery.json")
gcloud resource-manager tags bindings delete --parent="$CONTROL_TAG_PARENT" \
  --tag-value="$CONTROL_TAG_VALUE" --project="$APP_PROJECT" --location=global
```

Never detach the broker binding. Whole-project shutdown is the fallback if
individual recovery cannot be verified.

Prepare exact alert trigger/cleanup commands before executing them:

```sh
python3 enterprise/infra/gcp/sandbox/controls.py alert-plan --manifest "$RUN/manifest.json"
python3 enterprise/infra/gcp/sandbox/controls.py alert --manifest "$RUN/manifest.json" --case revision
python3 enterprise/infra/gcp/sandbox/controls.py alert-collect --manifest "$RUN/manifest.json"
```

Review `$RUN/alert-commands.json`. Execute separate `--case` invocations for
`run-service`, `run-job`, `compute`, `node-pool`, `service-iam`, `account-iam`,
`audit-config`, `nonbroker-read` and `broker-read`. The first four create bounded
sandbox-only resource names from the run ID; cleanup runs only after successful
creation. The node-pool has zero nodes. Audit configuration is restored in a
`finally` block using a fresh etag. Revision uses the reviewed broker digest and
the actual deploy identity, providing the authorized-deploy positive control.
No job execution or provider request occurs. Compute creates one private
`e2-micro` VM only for the audited service-account attachment, then deletes it.

`alert-collect` requires both exact managed policy names in the trigger project,
including `BYOK project: audit configuration or IAM changed` for BYOK triggers.
Missing or ambiguous inventory fails before querying logs. It uses each actual
policy ID and exact log filter within the recorded trigger timestamp window,
recording only policy/condition names and matched insert IDs. The owner must
independently confirm each expected test-sink notification with timestamp and
policy/event identifier; a log match is **not** a delivered notification.
`broker-read` is the negative alert control. Collect a bounded observation
window separately for that case and confirm no matching non-broker-read alert.
Missing receipts remain pending. Incomplete create/delete operations are recorded
as incomplete; use the generated cleanup argv after inspecting the named
resource, or shut down the two owned projects.

Protected WIF cases and jobs are owned by the coordinating implementation task.
Preserve ADR 0081's exact workflow, protected ref and Environment anchors. The
same-PR protected-ref-only jobs run after normal merge; existing protected
successful authentication proves only its own SHA/identity/claims. Neither
feature-branch success nor source-grep tests substitute for those results.
Assumption 9 remains deferred to #248.

### Final destructive undelete phase

Run this **after all other broker, payload, Pod, session and alert probes** and
after `restore-deny`. It requires the same run's successful deleted, untagged
control undelete from `lifecycle`. It temporarily soft-deletes the actual sandbox
broker as administrator; do not run application requests concurrently.

```sh
python3 enterprise/infra/gcp/sandbox/controls.py broker-undelete-last --manifest "$RUN/manifest.json"
# If interrupted or recovery remains pending, run only recovery next:
python3 enterprise/infra/gcp/sandbox/controls.py recover-broker --manifest "$RUN/manifest.json"
```

Before deletion, the phase durably saves the broker numeric UID, IAM policy and
actual tag bindings in `broker-undelete-recovery.json`. It verifies deletion and
observes whether the original tag values remain on the deleted identity, then
attempts deploy's `iam.serviceAccounts.undelete` against that exact UID. A 2xx
is failure; an exact permission/resource IAM denial passes only if the deleted
identity's tag state was observed unchanged. Lifecycle errors, unavailable tag
observations or tag loss remain inconclusive and block acceptance. Capture the
concrete live result and repair/review the tag/lifecycle design if that occurs;
nine other successful permission probes do not establish all ten permissions.

The `finally` recovery and the separately resumable `recover-broker` phase use
administrator credentials to undelete the same UID if needed, restore IAM with a
fresh etag, recreate only missing original tag bindings, and verify the enabled
UID, exact bindings and original tag-value set. Unknown additional tags refuse
completion. A failed recovery keeps `restore_pending`; never recreate an account
with the same email as a substitute. Read-only observations may be retried after
propagation; whole-project shutdown remains the final cleanup fallback. The
phase makes no provider request and changes no Terraform lifecycle guard.

## Cleanup

After probes, revoke dummy provider grants in their owner UI if any were used,
disconnect Plan Connections/Provider Credentials, delete private callback/token
artifacts, and record the boolean outcome. Dummy-secret IAM probes require no
real provider credentials. The envelope does not revoke the signed-in person's
ADC/gcloud login. Delete only run-specific credential files; preserve evidence
and state until shutdown verification succeeds.

```sh
python3 enterprise/infra/gcp/sandbox/runner.py cleanup --manifest "$RUN/manifest.json"
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
