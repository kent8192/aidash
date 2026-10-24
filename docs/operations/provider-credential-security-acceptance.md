# Provider credential security acceptance

This record separates implementation review, local regression checks and live
acceptance for [#135](https://github.com/kent8192/aidash/issues/135). The
[approved scope](provider-credential-security-completion.md) defines the required
observations. Passing local fixtures does not establish a provider or IAM result.

## Implementation and review

The delivery branch is `feat/issue-135-remaining-credential-security`, based on
`develop/0.1.0`. It includes the personal Plan Connection UI and Desktop relay,
Native Responses mapping and accounting, durable token-rotation recovery, hosted
route isolation, sandbox lifecycle/probes and protected-ref negative probe jobs.
Independent component and consolidated source reviews are complete. The final
receipt/pin-path review found no additional issues after the missing-pin recovery
guard in `5af597ffd6`.

The fresh development branch was merged normally at `246cb001d6f`, including
the Tenant tool endpoint restrictions. No rebase or force push was used.

## Local regression evidence

Observed on 2026-10-24 using Rust 1.96.0 and Node 26.3.1 on macOS. Build artifacts
and detailed logs are local working artifacts; the final PR records its delivered
revision and hosted check links separately.

| Check                                                   | Observation                            | Scope and limits                                                                                                                                                                   |
| ------------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Domain, Application, Harness and Integrations libraries | 6,722 passed; 1 ignored                | Integration run before the later Plan recovery changes and latest base merge; includes Native streaming/history and accounting regressions                                         |
| Plan Connection Application tests                       | 55 passed                              | Re-run at `ac19a07416`; missing-pin status/bearer regression first failed in both cases before repair                                                                              |
| Keyless dashboard model owner contract                  | 1 passed                               | Real Application register/persistence accepts the shared `openrouter` fixture and rejects the unsupported `openai` adapter name without writes                                     |
| Desktop relay                                           | 40 passed                              | Loopback validation, cancellation, expiry and task cleanup; expiry regression reproduced retained tasks before repair                                                              |
| Native Desktop bundle                                   | Built; local ad-hoc signature verified | Normal debug build without the `e2e` feature, isolated app identifier and validated Community production assets; no launch or provider consent                                     |
| Sandbox lifecycle and controls                          | 31 passed                              | Local subprocess/resource fixtures; no real permission outcome is inferred                                                                                                         |
| WIF/deploy negative probe contracts                     | 19 passed                              | Local contracts; protected-ref executions are pending                                                                                                                              |
| Runner isolation regressions                            | 62 passed                              | Existing local Runner gates                                                                                                                                                        |
| Terraform provider-mocked tests                         | 17 bootstrap and 31 environment passed | Validation and formatting also passed; no apply or live IAM proof                                                                                                                  |
| Community/Enterprise dependency boundaries              | Passed                                 | Production dependency graphs and shared dependency alignment                                                                                                                       |
| Final consolidated server/database tests                | 566 passed                             | Server library 475; execution authorization 44; Plan journey/recovery 21; Provider Credentials 25; remote grants 1. The later Clippy-only let-chain preserves the tested condition |
| Community backend Clippy                                | Passed                                 | `--workspace --all-targets --all-features -- -D warnings` at `ac19a07416`; Enterprise workspace remains a separate hosted gate                                                     |
| Actual Community API/dashboard                          | Passed                                 | Final binary export, Orval, TypeScript/Vite, 26 Node unit tests and 51 production-bundle browser tests                                                                             |
| Enterprise Offering and broker                          | 4 and 38 passed                        | Actual Enterprise startup guards; complete broker suite including correlated secret-free audits                                                                                    |
| Actual Enterprise API/dashboard                         | Passed                                 | Final binary export, Orval, TypeScript/Vite and 4 production-bundle browser tests; hosted Plan paths absent                                                                        |

The actual Community export contains four Plan Connection paths and five
operations; the Enterprise export contains none. Each dashboard built against
its own binary's schema. The supported export script restored the Community
generated client after the Enterprise checks.

## Live acceptance and Issue disposition

No disposable project was created, no billing account was linked and no real
provider consent was performed during the local checks above. The sandbox runner
and recipes are preparation for the following observations, not their evidence.

### Initial sandbox attempt (2026-10-24)

Subsequent owner-authorized execution created both disposable projects through
the sandbox runner's ownership journal. Billing was linked to the application
project. Google refused the BYOK project's billing link with
`FAILED_PRECONDITION / QuotaFailure: Cloud billing quota exceeded`. An additional
billing-linked project slot request was prepared, but transmission approval was
still pending at 03:46 UTC. Terraform planning/apply, private image builds and
real IAM or provider acceptance had not started.

The single-name DNS-01 certificate was issued and normal workstation resolution
of the new leaf returned only loopback. Temporary challenge TXT records were
removed. A new Google Web client for Community OIDC sign-in was created in the
application project after the owner completed the policy acknowledgement. The
Console and downloaded configuration confirmed External/Testing, one owner
test user, only `openid`/email/profile scopes, no JavaScript origins and the exact
approved callback. The retained secret files were verified to be owned and
mode-0600; the original Downloads copy was removed. No real provider consent was
performed.

At that observation, the new client, certificate, ACME account, remaining DNS
record and both projects still required cleanup. The original shutdown deadline
was 2026-10-25 02:54 UTC. A task-local cleanup watcher was armed with the exact
owned project IDs and numbers, source revision and lifecycle journal. It treats
an undeleted or unconfirmed Google client as incomplete cleanup while allowing
GCP teardown to proceed. It requires the workstation/process and authorizations
to remain available; its armed receipt is not shutdown evidence.

### Quota decision and verified teardown

The owner subsequently approved a request for one additional billing-linked
project slot. The first submission mistakenly entered `4` in the field for
additional projects, interpreting it as the total, although its prose requested
one additional slot. Google's official form acknowledged receipt, and the
specified mailbox received a quota-denial notification at 04:02:38 UTC. It gives
general review factors without an individual reason or project/case identifier;
the recipient, topic and immediate timing correlate it to this submission. The
cause of denial is unknown. No additional billing slot was granted.

The sandbox was then cleaned up before the original 24-hour deadline. Console
readback confirmed the new Google client absent from the active client list,
and its two private credential files were removed. The reviewed leaf helper
completed certificate revocation and ACME account deactivation. An independent
Cloudflare GET found no record at either exact A or challenge TXT name.
Independent Google API readback confirmed both owned projects in
`DELETE_REQUESTED`, with billing disabled and the billing-account association
empty. Google's recovery period and delayed final billing remain separate from
these observed shutdown results.

The cleanup watcher exited successfully after verifying all three resource
lifecycle records. Its AC-only sleep assertion also exited and was absent from
the system assertion inventory. After external cleanup was settled, the copied
DNS token and run TLS private key were removed. Ownership journals, sanitized
receipts and public certificate evidence remain available; private key retirement
is recorded separately, so do not replay certificate issuance or cleanup.

After cleanup, read-only billing observation found two billing-enabled projects.
A corrected support request entered `1` in the additional-project field, retained
the approved minimum capacity goal of four overall, and disclosed the first
input error and completed cleanup. Its full 990-character body was verified
before one submission attempt. Google's confirmation page acknowledged receipt
at 04:34 UTC. The corrected request remains pending; this acknowledgement is not
quota approval or a new sandbox attempt.

No Terraform plan/apply, private image build, real provider consent or live
IAM/GCIP/metadata/alert acceptance occurred. Their required evidence remains
pending. The quota refusal does not create an additional scoped deferral.

### Required live evidence

| Scope                                            | State                                 | Required evidence                                                                                                                                           |
| ------------------------------------------------ | ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| #136/#137 Cloud Store and broker                 | Pending                               | Actual create/manage/read split, prefix isolation, ingress, KMS verification and streamed provider calls                                                    |
| #138 GCIP                                        | Pending                               | Tenant-scoped lookup, disabled-user and `validSince` rejection using real identities                                                                        |
| #149 metadata isolation                          | Pending                               | Actual deployed workloads and their independent permitted/denied metadata paths                                                                             |
| #151/#152 deploy identity, IAM and alerts        | Pending                               | All ten broker deny permissions and tag lifecycle, authorized controls, correlated test-sink receipts and sandbox teardown                                  |
| #157 Self-hosted Store                           | Local database regressions passed     | Ciphertext, Master Key recovery and interrupted Plan grant cleanup passed in the consolidated Provider Credential/Plan Connection suites; no GCP dependency |
| #158 OpenRouter                                  | Pending                               | Owner-authorized OAuth, ordinary model/embedding/compaction calls and supported zero-data-retention paths                                                   |
| #159 Self-hosted ChatGPT plan                    | Pending                               | Owner-authorized cross-host code exchange, real model/tool stream, lifecycle and no-fallback behavior                                                       |
| #249 Native projection                           | Implemented; local regressions passed | ADR 0083 records the selected no-reasoning-retention design; real plan acceptance remains with #159                                                         |
| #248 hosted Plan Connections / #152 assumption 9 | Explicitly deferred                   | Re-enter after OpenAI approves the hosted app and its contract is known; Cloud/Dedicated endpoints and enablement remain disabled                           |
| Protected-ref WIF negatives                      | Pending after normal merge            | Exact merge SHA and workflow run URLs; do not weaken ref or Environment controls for a feature-branch rehearsal                                             |

Only #248's approval-dependent scope is deferred. Other pending entries remain
required. Keep #135 and children without acceptance evidence open. Protected-ref
acceptance follows the approved order and does not authorize automatic merge.

Sandbox teardown is complete only after observing billing unlinking and
`DELETE_REQUESTED` for both owned projects, plus required credential/IAM cleanup.
Google's recovery interval remains distinct from permanent deletion. Record any
incomplete restore or cleanup as pending and retain its resumable ownership journal.
