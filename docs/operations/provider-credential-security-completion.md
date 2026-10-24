# Complete Issue #135 in one pull request

Design status: approved on 2026-10-23, including the protected-ref acceptance
order. Implementation and verification remain tracked separately below.

The [acceptance record](provider-credential-security-acceptance.md) contains
observed local results and the remaining live evidence requirements.

Implementation baseline: `37d427e09b519fbede493d01997e2447472126dd` on
`develop/0.1.0`. Delivery branch: `feat/issue-135-remaining-credential-security`.

## Intent and completion

Finish the remaining Provider Credential, Plan Connection and Cloud identity
work under [#135](https://github.com/kent8192/aidash/issues/135) in one PR against
`develop/0.1.0`, preserving the decisions already recorded in the repository.
Completion requires working behavior, proportional regression checks and the
outstanding live acceptance evidence. A merged implementation or passing mock
test does not establish a live IAM or provider result.

The user selected two recommendations on 2026-10-23:

1. Explicitly defer only the hosted Plan Connection scope blocked by OpenAI
   approval (#248), including acceptance that cannot be performed until its
   hosted contract is known. Complete the other work through live acceptance.
2. Implement Native projection for ChatGPT plan models, and reject encrypted
   reasoning retention with reasons, as recorded in ADR 0083.

## Approaches

| Choice                                                                          | Trade-off                                                                                            | Decision                               |
| ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------- |
| Wait for hosted approval before completing the PR                               | Covers hosted execution, but delivery depends on an external approval and contract                   | Rejected for this delivery             |
| Record the approval-dependent deferral and finish the available scope in one PR | Leaves #248 open and disabled; permits independently verifiable work to finish                       | Selected by the user                   |
| Persist encrypted reasoning alongside Native turns                              | May improve continuity; adds provider-specific storage, retention, compaction and recovery contracts | Rejected by the user for this delivery |
| Map Native messages and tool items without reasoning retention                  | Preserves the current journal policy; forgoes encrypted-reasoning replay benefits                    | Selected by the user                   |

## Scope and acceptance inventory

GitHub's actual Sub-issue relation contains eleven children and no grandchildren.
The parent body omits #248/#249 and has stale #149/#158 checkboxes.

| Child | Baseline                                  | Remaining acceptance in this PR                                                                                        |
| ----- | ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| #136  | Closed; Store and bindings implemented    | Real create/manage versus payload-read IAM split through #152                                                          |
| #137  | Closed; broker implemented                | GKE internal ingress, prefix isolation, KMS signatures and streaming acceptance                                        |
| #138  | Closed; GCIP implemented                  | Real Tenant-scoped account lookup, disabled users and `validSince` behavior                                            |
| #149  | Closed; metadata policies implemented     | Probe the current GKE policies' denied and permitted paths                                                             |
| #151  | Open; implementation merged in #210       | Deny/tag, protected entry point and alert-delivery acceptance; repair proven gaps                                      |
| #152  | Open; no recorded sandbox result          | Positive and negative IAM/identity probes, evidence and sandbox teardown                                               |
| #157  | Closed; PostgreSQL AEAD Store implemented | Preserve Community build, recovery and ciphertext regression checks                                                    |
| #158  | Closed; OAuth-only path implemented       | Real OpenRouter authorization and supported ZDR-path evidence; update stale documentation                              |
| #159  | Open; server phases implemented           | Personal UI, Desktop relay, model selection, status correctness, refresh recovery, docs and real connection acceptance |
| #248  | Open; external approval dependency        | Record the exact scoped deferral and re-entry conditions; keep hosted endpoints disabled                               |
| #249  | Open; Native currently rejected           | Native Responses message/tool mapping, request estimates and ADR 0083                                                  |

Existing licenses and placement are settled by ADRs 0032/0035 and #124. Community
owns portable contracts; the broker and GCP adapters belong to Enterprise. The
independent license cutover and infrastructure relocation are not reopened here.

## Plan Connection behavior

Complete the Self-hosted behavior in ADRs 0007/0031. Enabling plan usage remains
an explicit server configuration choice, off by default. The personal settings
screen explains that whoever operates the host can read Plan Tokens and that
context compaction uses the Tenant's OpenRouter binding.

The screen supports connect, reconnect, disconnect, connection status and a
Manage usage link. It belongs to a subject's personal settings, not Deployment
Administrator settings. Only the owning subject's session sees its connection;
metadata exposes no tokens, client ID or provider account identifiers.
Status must reflect the current connecting External Identity and enabled
Mappings, including identity disablement, replacement and multiple-identity
ineligibility. Configuration disablement prevents new use while retaining
status and disconnect. A persistent dashboard banner and the affected Run
explain unusable or reconnect-required states without exposing tokens.

The server retains `state`, `nonce` and the PKCE verifier and alone exchanges
the authorization code. Browser users may paste the loopback callback URL;
this is a single-use code, not Key Material. Desktop binds a loopback listener
on `127.0.0.1` with an available port, opens the provider authorization and
relays the callback over the same authenticated Desktop session. The backend
enforces pending-flow/session ownership and one-time consumption. Neither
client receives or uploads Plan Tokens in this design.

The Desktop listener waits without holding the IPC operation mutex needed by
authenticated API transport. Tenant, Mapping or session changes and logout
cancel the listener and pending UI; late responses cannot attach to another
subject. Callback URLs remain in transient memory only.

Before completing the relay, perform the ADR 0031 spike: authorize in a browser
and exchange on a different Self-hosted server. If OpenAI rejects the exchange,
record the response and stop this path for a targeted design amendment. Do not
silently introduce token-upload endpoints or present mocked exchange as success.

Model forms and displays distinguish Tenant Provider Credentials, personal Plan
Connections and Keyless endpoints. Eligible model discovery and Run admission
remain server-authorized. A registry model does not grant another subject use
of a personal plan. Preserve owner-rooted delegation, local-only execution,
revocation, allowance exhaustion and the no-fallback rule.

## Token rotation and recovery

Address the outstanding [#305 review](https://github.com/kent8192/aidash/pull/305#pullrequestreview-5556115225)
about a crash between storing refreshed tokens and updating their metadata pin.
Use the Self-hosted PostgreSQL Store to make the refreshed local version and
connection pin recoverable together, while preserving per-connection refresh
serialization. The implementation may choose a shared database transaction or
a durable recovery protocol; it must demonstrate the failure windows.

The upstream token refresh cannot be made atomic with a local transaction. If
the process loses the replacement before local durability, fail closed and
require reconnection rather than repeatedly replaying an invalid old grant.
Disconnect and identity/Tenant lifecycle cleanup must account for every locally
retained token version, including interrupted rotations. Never leave a newer
grant usable while reporting a completed disconnect. Distinguish confirmed
upstream revocation from local deletion when the provider cannot be reached.

## Native Responses mapping

Map Native messages in order to Responses input items, including text, supported
images, `function_call` and matching `function_call_output` items. Preserve tool
call IDs, names and arguments across multiple tools, retries and recovery.
Continue to reject unsupported input explicitly. Preserve streaming output,
cancellation and bounded resource handling.

Request Estimates and Stable Prefix handling use the serialized Native request.
Store no reasoning output and request no encrypted reasoning replay. Validate
multi-step tool execution and resumed Runs in addition to a single completion.
Legacy and Ordered projection behavior remains covered by regression checks.

## Cloud sandbox and evidence

Use a dedicated disposable application project and a distinct BYOK project,
without customer data. Existing production and nonproduction projects are not
disposable. Their public configurations currently have no enabled BYOK project
or broker, and the broker image manifest has no approved digest.

Prepare reviewable Terraform variables/plans and a bounded probe/cleanup runner
before cloud mutations. Project IDs, billing scope and permission to provision
the concrete sandbox must be established at that point. Use dummy secret
payloads for all #152 probes; never provider keys or customer credentials.

| Assumption | Required evidence                                                                                                                                                                                           |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1          | Deploy grants only the fixed Create/Manage roles; other grants and audit-configuration escalation fail                                                                                                      |
| 2          | Project-number prefix conditions allow the intended management/read calls and reject other prefixes                                                                                                         |
| 3          | Runtime, server, worker and deploy cannot read payloads; each broker reads only its environment                                                                                                             |
| 4          | Current GKE Pods reach the internal broker; an internet caller cannot; relevant workloads cannot bypass metadata policy                                                                                     |
| 5          | Authorized KMS Ed25519 signing verifies against the exported public key; unauthorized signing fails                                                                                                         |
| 6          | Tenant-scoped GCIP viewer lookup succeeds only in the permitted Tenant without project-wide user access                                                                                                     |
| 7          | Record which account changes advance `validSince`, and verify resulting session/account denial behavior                                                                                                     |
| 8          | Broker-targeted deny covers the current ten permissions and tag lifecycle; authorized deployment works; forbidden WIF entry points fail; revision/IAM/non-broker read alerts reach the configured test sink |
| 9          | Hosted Plan-secret refresh scope belongs to #248; record its approval-dependent deferral and do not claim it verified                                                                                       |

Preserve ADR 0081's current nonproduction/production trust anchors. Verify
GitHub Environment and WIF behavior against current deployment code rather than
the stale `gcp-deploy` wording in ADR 0005. Determine whether observed reviewer
settings meet the accepted trust boundary and fix only an evidenced mismatch.

Record the revision, command/probe identifier, identity, target, expected and
observed status, timestamps and sanitized response metadata. Do not record
payloads, authorization codes, tokens, cookies or raw provider bodies. A
rejected assumption is a design failure to repair and re-probe, or a separately
approved explicit deferral, not a successful probe.

Design teardown for protected KMS/state resources before provisioning. A
sandbox-only project envelope owns only the two new projects and cannot adopt
an existing one. It disposes of them through project shutdown without changing
production `prevent_destroy` protections. Record credential cleanup, billing
unlinking and the projects' `DELETE_REQUESTED` state. Google retains a recovery
period before permanent deletion; label that pending state accurately rather
than claiming immediate permanent removal. A requested cleanup without observed
shutdown is not teardown evidence.

Real OpenRouter and ChatGPT acceptance uses the Provider Authorization screens
and the owner's interactive consent. Keep all resulting credentials inside the
configured Store and redact callback URLs from artifacts. Browser consent may
require the owner; no acceptance is inferred from a fake provider or a past PR.

## Validation and delivery

Use focused application, adapter, database, browser and Desktop checks for the
changed behavior. Include meaningful tests of interrupted rotation, session
binding, ownership, Native tool history, cancellation and secret non-disclosure.
Run the existing authorization, federation, recovery, Runner isolation and
Community/Enterprise boundary gates relevant to these changes. Consolidate
expensive checks after implementation; broaden only for failures or new risk.

### Protected-ref acceptance order

Order accepted by the user on 2026-10-23:

1. Before merge, finish implementation and regression checks and perform the
   live sandbox and provider probes that do not need the new code on a protected
   GitHub ref. Keep their sanitized evidence in the PR. Prepare the protected-ref
   probe jobs and their exact acceptance/cleanup steps in this same PR.
2. If a probe must run from the protected workflow ref, execute it after this
   PR is merged through the normal repository process. Do not weaken WIF or
   Environment restrictions to make a feature-branch run look like that proof.
   Record the source/merge SHA, run URL, identities and outcomes on the PR and
   relevant Issues. This design decision is not permission to merge an unseen PR.
3. Until those results succeed, explicitly report acceptance pending and keep
   #135 and the affected children open. Repair failures before claiming
   completion. If post-merge source corrections prove necessary, report that
   blocker rather than silently promise that an additional PR is unnecessary.

The one implementation PR contains code, ADRs, updated
Cloud/Self-hosted/OAuth/upgrade documentation, the sandbox runner and evidence
available before merge. Post-merge acceptance records are attached to the same
PR and Issues without requiring another documentation-only PR. Reconcile #135's
inventory and the children with exact evidence and the #248 deferral. Keep local
checks, exact delivered SHA, review closure, hosted CI and live acceptance as
separate claims. Do not close a child lacking its evidence or the explicitly
accepted external deferral.

## Builder choices and remaining prerequisites

The builder selects file organization, UI component composition, internal
transaction/recovery implementation, fixture layout and probe orchestration
within these contracts. Use existing project conventions and architectural
dependency boundaries.

Remaining external prerequisites are the concrete disposable-project/billing
scope, required cloud permissions and the owner's provider consent. These are
execution prerequisites; they do not change the selected product behavior.
The complete design is reviewed before product implementation begins.

## Sources

- [Parent Issue #135](https://github.com/kent8192/aidash/issues/135) and its
  eleven actual children; [#248](https://github.com/kent8192/aidash/issues/248)
  records the hosted approval dependency and
  [#249](https://github.com/kent8192/aidash/issues/249) permits a reasoned decision
  against reasoning retention.
- [OpenAI's open-source integration guide](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt)
  distinguishes locally running open-source integrations from paid or remotely
  hosted apps that must request access.
- [Google project deletion and restoration](https://docs.cloud.google.com/resource-manager/docs/delete-restore-projects)
  explains the shutdown and recovery interval; cleanup evidence must distinguish
  shutdown from permanent deletion.
