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

| Check                                                           | Observation                            | Scope and limits                                                                                                                               |
| --------------------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Domain, Application, Harness and Integrations libraries         | 6,722 passed; 1 ignored                | Integration run before the later Plan recovery changes and latest base merge; includes Native streaming/history and accounting regressions     |
| Plan Connection Application tests                               | 55 passed                              | Final recovery implementation; missing-pin status/bearer regression first failed in both cases before repair                                   |
| Keyless dashboard model owner contract                          | 1 passed                               | Real Application register/persistence accepts the shared `openrouter` fixture and rejects the unsupported `openai` adapter name without writes |
| Desktop relay                                                   | 40 passed                              | Loopback validation, cancellation, expiry and task cleanup; expiry regression reproduced retained tasks before repair                          |
| Sandbox lifecycle and controls                                  | 31 passed                              | Local subprocess/resource fixtures; no real permission outcome is inferred                                                                     |
| WIF/deploy negative probe contracts                             | 19 passed                              | Local contracts; protected-ref executions are pending                                                                                          |
| Runner isolation regressions                                    | 62 passed                              | Existing local Runner gates                                                                                                                    |
| Terraform provider-mocked tests                                 | 17 bootstrap and 31 environment passed | Validation and formatting also passed; no apply or live IAM proof                                                                              |
| Community/Enterprise dependency boundaries                      | Passed                                 | Production dependency graphs and shared dependency alignment                                                                                   |
| Final consolidated server/database tests                        | Pending                                | Plan journey/recovery, Provider Credentials, execution authorization and remote grants                                                         |
| Actual Community/Enterprise API generation and dashboard builds | Pending                                | Must use each final binary's own schema, including hosted Plan-path exclusion                                                                  |

## Live acceptance and Issue disposition

No disposable project was created, no billing account was linked and no real
provider consent was performed during the local checks above. The sandbox runner
and recipes are preparation for the following observations, not their evidence.

| Scope                                            | State                                            | Required evidence                                                                                                                                    |
| ------------------------------------------------ | ------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| #136/#137 Cloud Store and broker                 | Pending                                          | Actual create/manage/read split, prefix isolation, ingress, KMS verification and streamed provider calls                                             |
| #138 GCIP                                        | Pending                                          | Tenant-scoped lookup, disabled-user and `validSince` rejection using real identities                                                                 |
| #149 metadata isolation                          | Pending                                          | Actual deployed workloads and their independent permitted/denied metadata paths                                                                      |
| #151/#152 deploy identity, IAM and alerts        | Pending                                          | All ten broker deny permissions and tag lifecycle, authorized controls, correlated test-sink receipts and sandbox teardown                           |
| #157 Self-hosted Store                           | Implemented; final local database checks pending | Ciphertext, Master Key recovery and interrupted Plan grant cleanup in the consolidated Provider Credential/Plan Connection suites; no GCP dependency |
| #158 OpenRouter                                  | Pending                                          | Owner-authorized OAuth, ordinary model/embedding/compaction calls and supported zero-data-retention paths                                            |
| #159 Self-hosted ChatGPT plan                    | Pending                                          | Owner-authorized cross-host code exchange, real model/tool stream, lifecycle and no-fallback behavior                                                |
| #249 Native projection                           | Implemented; local regressions passed            | ADR 0083 records the selected no-reasoning-retention design; real plan acceptance remains with #159                                                  |
| #248 hosted Plan Connections / #152 assumption 9 | Explicitly deferred                              | Re-enter after OpenAI approves the hosted app and its contract is known; Cloud/Dedicated endpoints and enablement remain disabled                    |
| Protected-ref WIF negatives                      | Pending after normal merge                       | Exact merge SHA and workflow run URLs; do not weaken ref or Environment controls for a feature-branch rehearsal                                      |

Only #248's approval-dependent scope is deferred. Other pending entries remain
required. Keep #135 and children without acceptance evidence open. Protected-ref
acceptance follows the approved order and does not authorize automatic merge.

Sandbox teardown is complete only after observing billing unlinking and
`DELETE_REQUESTED` for both owned projects, plus required credential/IAM cleanup.
Google's recovery interval remains distinct from permanent deletion. Record any
incomplete restore or cleanup as pending and retain its resumable ownership journal.
