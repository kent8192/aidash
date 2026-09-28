# Issue 38: scoped remote execution design review

Status: the process-crash evidence gap identified during review is now covered by the local Golden Path verification below. Issue 38 was observed closed on GitHub on 2026-09-28. The interview questions and recommendations below are not accepted decisions or additional Issue 38 acceptance conditions.

## Golden Path verification

The updated gate was built and run against `20d1fb46f13cf042af6afce7763140c132428b78` plus the local acceptance changes. `python3 scripts/golden_path.py --binary /Users/kent8192/target/debug/aidash --dashboard` exited successfully. The actual run used an isolated Compose project and ports, with two independent Node servers, PostgreSQL, NATS and real Worker subprocesses.

- Four required subject-scoped scenarios passed: Worker SIGKILL after a committed Home message, after committed Home completion, peer outage and recovery, and grant revocation while the Worker was stopped.
- The four scenarios retained exactly four admissions and four Home bindings, the original Run identities, and unique committed outputs. Revocation prevented subsequent inference and tool effects, rejected resume, and allowed authorized cancellation.
- Cross-tenant/workspace access, unapproved dependencies, disabled Agents and legacy admission were denied. Subject credentials were not forwarded to peer or provider requests.
- Existing legacy federation, NATS outage recovery and transport checks passed. All 130 Chromium tests passed.
- Rust formatting, Clippy for the query helper, and Trunk checks for the changed acceptance files passed.

The generated `.ignore/acceptance/report.json` records source fingerprint `8ff3b1c3b8032e7254b3eab4c5473dd4c6252888d21d63fb30bb9fc461ff97ff` and binary SHA-256 `0cffdc8dbade4bdcc38191485865341320ad7ccf4a59ed18d33fc066d84d491b`, along with distinct before/after Worker PIDs. This documentation update followed that run; no executable test or application code changed afterward. The new tests remain local and are not evidence of a hosted CI run. CI already calls `scripts/test-acceptance.sh`, which builds the checkout and now runs the scoped scenarios unconditionally.

The remainder records the earlier review and its historical evidence limits.

## Basis

- [Issue 38](https://github.com/kent8192/aidash/issues/38), inspected on 2026-09-28, remains open with six unchecked acceptance criteria.
- [Functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24?pvs=204), fetched on 2026-09-28: FR-FED-004 and FR-AUTH-001. FR-TX-001 defines a separate distributed-transaction contract; ordinary external tools retain their durable-execution contract.
- Inspected source: `236bc28b099dcd77a6ae8df7608f84028d9b1fec` on `develop/0.1.0`, the merge of [PR 45](https://github.com/kent8192/aidash/pull/45) on 2026-09-27.
- Design checkout: `docs/issue-38-scoped-federation-design`.

The Issue's premise predates PR 45. Receiver activation, scoped Home commands and a remote-execution dashboard are present in this source. Several passages in [authorization](../authorization.md) and [protocol](../protocol.md) still describe activation and Home commands as unavailable. The [operations guide](../operations/core-capabilities.md) describes their implemented scope. Neither code presence nor an old CI report establishes that all Issue 38 acceptance criteria are satisfied.

## Observed implementation and evidence

| Issue acceptance area                                  | Current source evidence                                                                                                                                                                                                                                                                            | Evidence limit                                                                                                                                                                                                                                |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Execute an exact admitted Agent and complete at Home   | [Source activation](../../src/authorization/remote/execution.rs), [receiver activation and Worker lease](../../src/authorization/peer/admission.rs), [Home commands](../../src/authorization/remote/execution/commands.rs); `scoped_remote_worker_finishes_at_home_and_retries_keep_one_execution` | Tests were inspected, not executed in this interview.                                                                                                                                                                                         |
| Independent source and receiver authority              | [Source grant validation](../../src/authorization/remote.rs), [receiver inspection](../../src/authorization/peer/execution.rs), [execution guard](../../src/authorization/execution.rs)                                                                                                            | Full credential-disclosure and authority-race coverage still needs an explicit acceptance mapping.                                                                                                                                            |
| Deny changed or withdrawn authority                    | Parameterized cases for source/receiver policy, expiry, grant revocation, source credential, receiver mapping, Task revision, disabled Agent and definition substitution in [scoped execution tests](../../tests/scoped_remote_execution.rs); an in-flight inference revocation case               | The meaning of revocation for already dispatched external effects must be settled.                                                                                                                                                            |
| Preserve one admission, Run and effect through retries | Durable execution bindings and command receipts; lost Home-effect reply and outage/reconnection cases in [scoped execution tests](../../tests/scoped_remote_execution.rs)                                                                                                                          | The test named `peer_outage_and_both_node_restarts_reconcile_one_scoped_execution` rebuilds pools, Federation objects and HTTP server tasks inside a test process. It is not, by itself, evidence of separate Node-process death and restart. |
| Reject unauthorized scope and legacy admission         | [Peer admission](../../src/authorization/peer/admission.rs), [grant tests](../../tests/remote_grants.rs), [admission tests](../../tests/remote_admission.rs), [peer authorization tests](../../tests/peer_authorization.rs)                                                                        | Every negative acceptance case needs to be associated with a concrete assertion before closure.                                                                                                                                               |
| Operator visibility without credentials                | [Remote-execution task panel](../../web/src/collaboration/remote-executions.tsx), source status API and sanitized receiver status                                                                                                                                                                  | Presence of UI code does not establish browser behavior or sufficient failure explanations.                                                                                                                                                   |

The dedicated suite is [tests/scoped_remote_execution.rs](../../tests/scoped_remote_execution.rs); [scripts/test-capability-runtime.sh](../../scripts/test-capability-runtime.sh) includes it in the runtime gate. Runtime tests and CI were not rerun locally for this documentation-only interview. Existing hosted CI evidence was subsequently inspected as recorded below.

## Closure assessment on 2026-09-28

The user authorized closing Issue 38 if it is ready. No Issue state or comments were changed because its restart acceptance is not fully evidenced by the inspected tests.

- [CI for the PR 45 merge](https://github.com/kent8192/aidash/actions/runs/36287327996) succeeded at `236bc28b099dcd77a6ae8df7608f84028d9b1fec`.
- [A later completed CI run](https://github.com/kent8192/aidash/actions/runs/36416005707) succeeded at `8c8b10f05a89e50b3ad99f9dc8444a331c9abd25`. Its downloaded `core-runtime-evidence` artifact contains 16 successful cases from `scoped_remote_execution.rs`, including authority withdrawal, lost effect replies and outage/reconnection.
- The latest remote branch observed during this assessment was `20d1fb46f13cf042af6afce7763140c132428b78`; [its CI](https://github.com/kent8192/aidash/actions/runs/36421041392) was still running. This is distinct from the successful earlier revisions.
- The scoped restart case reconstructs database pools and Federation objects and restarts HTTP server tasks inside the same process, after a completed Worker step. It does not terminate an executing scoped Worker process.
- [Cluster acceptance](https://github.com/kent8192/aidash/blob/20d1fb46f13cf042af6afce7763140c132428b78/scripts/cluster_acceptance.py) does terminate a Worker Pod and check recovery, but uses the operator-token Golden Path. It does not provision the subject credentials, peer mappings, remote grants and admissions needed to exercise the scoped execution path.

Consequently, successful scoped tests and successful legacy-process recovery tests must not be combined into a claim that scoped execution has passed process-crash recovery. The outstanding evidence should run a subject-authorized remote assignment on independent Node processes, terminate its Worker during execution, restart it, and verify that it preserves the admission/Run binding, does not duplicate Home effects, and rechecks current authority before continuing. This is a validation gap, not an observed runtime failure.

The four interview questions below concern possible contract clarification or expansion. In particular, remote core working-area capabilities and grant renewal are not prerequisites invented for closing the original Issue.

## Existing boundaries that affect the interview

- Remote grants have an immutable preparation expiry between 1 and 3,600 seconds. The convenience delegation path requests 3,600 seconds. Retrying preparation does not extend the expiry.
- Receiver admission pins the source grant, Task, mapped credential, subject chain and execution definitions. A different grant cannot silently replace the existing admission for a Task.
- Remote Agents can use the scoped Workspace execution path. Core working-area capabilities require an admitted local thread and are rejected before remote admission; independently owned files can be exchanged through the separate scoped file-transfer path.
- Current tests expect revocation during an outstanding model response to pause execution, prevent Home outputs and reject resume under the revoked grant, while authorized cancellation can close both sides.
- Home-command receipts bind an idempotency key to the operation and input. Reusing the key with different input conflicts. This is distinct from claiming arbitrary external effects are exactly-once.

## Design tree and first-round frontier

All four root decisions are open. Their child decisions will be asked only after their prerequisite is settled.

### Q1: remote capability scope

Should Issue 38 completion retain the existing remote Workspace capability boundary, or must remote Agents also run core Shell/Python/working-file capabilities?

Recommendation: retain the Workspace boundary for Issue 38 and validate every original acceptance criterion against it. Extending core working-area execution requires an explicit ownership and isolation design.

Dependent decisions: if the scope expands, who owns the execution working area, which identity authorizes file access, and how local-thread and remote-Task lifetimes relate; otherwise, capability rejection and user-visible explanations.

### Q2: work that outlives a grant

Must a Task that waits or runs beyond the current one-hour ceiling be resumable as the same Run after explicit reauthorization, or is expiry a hard limit requiring a separate new work request?

Recommendation: support explicit reauthorization of the same Task/Run while preserving old authority records and previously committed effects. An expired grant must never itself authorize continued execution. This is a proposed extension, not existing behavior.

Dependent decisions: eligible reauthorizers, renewable versus terminal reasons, authority epochs, immutable bindings, expiry budgets, duplicate renewal and recovery after either Node stops during reauthorization.

### Q3: revocation and an in-flight external effect

If an external request has already been dispatched when authority is revoked, does the contract require immediate remote cancellation/rollback, or prevention of subsequent protected actions and result delivery with explicit treatment of the outstanding effect?

Recommendation: define the authorization cutoffs at durable boundaries, prohibit later protected effects and Home delivery after failed revalidation, and request cancellation where supported. An uncertain non-idempotent external effect requires reconciliation; it must not be silently replayed or described as rolled back.

Dependent decisions: precise ordering of revocation versus dispatch/commit, authority held across a boundary, handling of already committed receipts, pending output retention, and the reconciliation UI.

### Q4: recovery from a temporary authority-service outage

If connectivity fails briefly while the same grant remains unexpired, and both Nodes later confirm the same authority and definitions, should the Run continue automatically or wait for a person to resume it?

Recommendation: allow bounded automatic recovery only for transient unavailability after both Nodes revalidate current authority and exact bindings. Revocation, expiry or changed identities/definitions must not be treated as a transient connectivity failure; uncertain unsafe effects need reconciliation.

Dependent decisions: failure taxonomy, retry timing and limits, operator controls, status explanations, and proof of restart convergence.

## Documentation during the interview

[CONTEXT.md](../../CONTEXT.md) records established domain terms. No new ADR has been accepted: the first round has no user answers yet. Record an ADR when an answered question establishes a consequential trade-off; keep unresolved alternatives in this review.

The existing `docs/adr/0001` through `0003` cover Web research. The next available number in this checkout is `0004`; verify it again before creating an ADR.
