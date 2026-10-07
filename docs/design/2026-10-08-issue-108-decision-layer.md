# Issue #108 decision layer design interview

Status: design confirmed and implementation requested on 2026-10-08; all 22 decisions settled. Q1-Q21 were accepted on 2026-10-08. Q22 was accepted with the user's explicit correction, "No limits; all other recommendations."

The requested deliverable is a design interview with a glossary and ADRs as decisions settle. This document records the decision tree and verified facts; implementation was subsequently authorized by the user ("OK, start implementation").

## References and baseline

- [Issue #108](https://github.com/kent8192/aidash/issues/108): decision definitions, DecisionGate, and decision log.
- [Issue #103](https://github.com/kent8192/aidash/issues/103): parent architecture and open threshold/state-storage decisions.
- Baseline: `origin/develop/0.1.0@015a8a4e9cb95ebf4ad93c1c279015c76bac7f48`, fetched on 2026-10-08.
- Implementation branch: `feat/issue-108-decision-layer` in the reused design worktree.
- [PR #129](https://github.com/kent8192/aidash/pull/129) merged the capability foundation. [PR #133](https://github.com/kent8192/aidash/pull/133), the native Binding cutover, is still open. Issue #106 is closed, but that alone does not establish native execution readiness.
- Existing vocabulary: [Registry capability glossary](../operations/registry-capability-glossary.md).
- Accepted decision vocabulary: [CONTEXT.md](../../CONTEXT.md).
- ADRs: [declaration and authority boundary](../adr/0001-separate-declarations-from-decision-authority.md), [explicit Decider cutover](../adr/0002-replace-compactor-with-explicit-decider-registration.md), [decision evidence and replay](../adr/0003-retain-decision-evidence-for-branch-replay.md), [one pinned Decider per hook](../adr/0004-pin-one-decider-per-hook-in-run-bindings.md), [accounting and atomic application](../adr/0005-account-attempts-before-dispatch-and-commit-decisions-atomically.md), and [restricted evidence and expiring state](../adr/0006-restrict-decision-evidence-and-expire-opt-in-state.md).
- Round 3 ADRs: [Execution-node dispatch](../adr/0007-keep-decision-dispatch-on-the-execution-node.md), [model and historical rule pins](../adr/0008-pin-external-model-versions-and-historical-branch-rules.md), and [no fixed decision limits](../adr/0009-impose-no-fixed-decision-size-or-cardinality-limits.md).
- Reviewable local drafts: [Issue #103/#108 amendments](2026-10-08-issue-108-amendments.md).

## Verified current behavior

- [CompactorConfig](../../crates/aidash-domain/src/registry/contracts.rs) identifies an external provider, endpoint, model, credential reference, and request/response limits. There is no generic DecisionGate on this baseline.
- [Compaction](../../crates/aidash-application/src/context.rs) only replaces the original context after the complete candidate fits. Failure leaves the original context intact; there is no summarization fallback. Calls are unnecessary when the original context already fits.
- [Compaction classification](../../crates/aidash-application/src/context/compaction.rs) preserves the first and recent events, fits a bounded classification view, uses independent call/result Noul questions, splits requests to fit, and applies a keep threshold of 0.5. Tool outputs remain available in the execution journal.
- [Generated compaction reservation](../../crates/aidash-application/src/generation/compaction.rs) requires every generated ancestor to approve the same exact compactor reference. It checks current authority and catalog permissions and commits each ancestor's attempt charge before provider I/O. Failed, uncertain, and crashed calls remain charged; a new attempt consumes new allowance.
- [Generation operations](../generation.md#approved-compaction) distinguish generated-chain compaction from ordinary Agents using the Node environment. The attempt ledger records provider, Run, bytes, and question count without request history or credentials.

## Decision tree

The first frontier comprised six independent roots. All six recommendations below have been accepted; their dependent details are settled in the subsequent rounds.

| Question | Decision | Accepted recommendation | Status |
| --- | --- | --- | --- |
| Q1 | Delivery scope | Implement the common decision contract and executable Compaction hook first; other hooks require later supported implementations. External-write automation belongs to #109 and model routing remains excluded. | Accepted |
| Q2 | Provider coupling | Keep the decision contract independent of Jev; supply Jev as the first probability-only adapter with exact versioned dependencies. | Accepted |
| Q3 | Threshold ownership | Keep invariant authority and safety rules in operator code; version action thresholds with the immutable decider definition. Agent/Node restrictions can only narrow behavior. | Accepted |
| Q4 | Compatibility | Use explicit new decider registrations without legacy compactor/configuration projection or automatic migration; revise #108's compatibility acceptance criteria. | Accepted |
| Q5 | State storage | Retain a state digest and access-controlled decision evidence by default; make full decision-state retention an explicit opt-in with separately settled permissions and retention. | Accepted |
| Q6 | Meaning of replay | Guarantee deterministic branch evaluation from recorded validated answers and exact effective rules; treat a fresh provider inference as a diagnostic experiment, not guaranteed reproduction. | Accepted |

### Subsequent branches

- Q1 -> executable-hook vocabulary, Compaction failure behavior, Shadow/Enforce semantics, activation criteria, and the dependency on native Binding cutover.
- Q2 -> versioned state-builder identity, option-source identity, request grouping, typed answer validation, actual provider constraints, and provider disclosure/authority rules.
- Q3 -> effective restriction combination, confidence bands, tie/boundary behavior, and the rules required in replay evidence.
- Q4 -> new registration and policy reference shape, ordinary-Agent provider selection, generated-chain budget representation, remote execution pins, and deployment cutover.
- Q5 -> evidence fields, removal of fixed payload ceilings, reader authority, sensitive option descriptions, opt-in full-state retention, deletion, and outcome linkage.
- Q6 -> decision/attempt identity, crash recovery, atomic application and journal persistence, replay fixtures, and validation criteria.

Each round will settle only decisions whose prerequisites are known. Accepted domain terms will be recorded immediately in `CONTEXT.md`; consequential architectural trade-offs will be recorded in `docs/adr/`.

## Round 2 frontier

Q1-Q6 unblocked the following decisions. All ten recommendations were accepted in the second round.

| Question | Decision | Accepted recommendation | Status |
| --- | --- | --- | --- |
| Q7 | Compaction failure and modes | Enforce applies Compaction only after the complete candidate fits; Shadow records a candidate without changing context. No provider call is needed when context fits. Unavailable, forbidden, invalid, or insufficient decisions preserve the original context and report the context-budget failure; no summarization or implicit switch to Enforce. | Accepted |
| Q8 | Binding and decision cardinality | Use an explicit Decider Binding, at most one per executable hook in an Agent. Pin its qualified definition, provider contract, model/endpoint configuration, builder, and effective restrictions in the Run snapshot. Reject duplicate hook bindings and unsupported hooks rather than applying precedence or resolving latest. | Accepted |
| Q9 | State and option construction | Support a versioned Node-owned Compaction builder and history-event option source first. Build counts and comparisons in code, authorize inputs, and require descriptions and stable revision-scoped option identities. Other builders and Static/Bindings/Live option sources require explicit supported implementations; declarations cannot supply executable code or undeclared reads. | Accepted |
| Q10 | Live authority and call accounting | Apply catalog, invocation, and external-disclosure authority before all provider requests, including Shadow. All Runs require an explicit call allowance; generated chains retain exact-version agreement and atomic charges across all ancestors. One physical request is one charged attempt, including each batch, failure, uncertain call, and retry; zero provider calls consume no allowance. | Accepted |
| Q11 | Compaction branching and narrowing | Preserve the current 0.5 keep threshold, first event, recent six events, and keep/truncate/drop ordering in the first version. A keep-result answer at or above the threshold retains the event; otherwise a keep-call answer at or above it truncates the result; otherwise the paired event drops. Narrowing may retain more history or forbid changes, never increase deletion. | Accepted |
| Q12 | Answers and batching | Execute Noul questions first with finite probabilities in [0,1]; unsupported question types reject before invocation. Require exact question coverage and validate complete batches before using any answers. Batch independent questions only for the same state, exact provider configuration, authority scope, and mode; split under actual external-provider constraints, without Aidash fixed byte/question ceilings (Q22). Any invalid or failed batch prevents the entire Compaction candidate from being applied. | Accepted |
| Q13 | Evidence and reader authority | Retain exact candidate identities/descriptions, validated answers, declaration/builder/rule versions and digests, effective restrictions, mode, branch, reason, and attempt/outcome links. Detailed evidence requires both Run access and dedicated decision-evidence authority, with applicable source restrictions; ordinary events carry only a safe summary. Raw provider bodies and credentials are excluded. | Accepted |
| Q14 | Full-state retention | Full-state recording is disabled by default and requires explicit tenant-authorized Node policy before evaluation. Default opt-in retention is seven days with a thirty-day ceiling; stricter source limits and revocation govern access. Evidence follows journal retention; full-state expiry leaves an explicit expired reference. Mandatory redaction occurs before disclosure and hashing, so a retained state digest identifies the exact permitted provider input. | Accepted |
| Q15 | Persistence and recovery | Commit attempt identity, exact request digest, and budget charges before provider I/O. Atomically commit an applied branch, its evidence, and updated Run context under worker/revision fencing; record failures without applying context. Recover only from durably stored valid answers for the same decision input and current permitted rules. Unrecorded responses remain charged and uncertain; a new HTTP attempt gets a new identity. Link later outcomes through appended events. | Accepted |
| Q16 | Delivery and cutover | Target native Binding execution after PR #133 integration; use no legacy fallback. Include registration/policy APIs, bilingual existing administration forms and Run-journal summary/evidence views. Require focused pure-contract checks and native local/generated/remote, crash, revocation, and UI acceptance. Drain old work before the breaking cutover and retain historical journals. Draft #103/#108 changes locally for final design review. | Accepted |

### Facts shaping Round 2

- Compaction changes a temporary candidate and only applies it when the full inference request fits; its current failure behavior is already atomic.
- The current classification keeps events when their probability is at least 0.5. Lowering this keep threshold preserves more content, so numerical threshold direction alone cannot define stricter restrictions.
- Existing Run Binding snapshots already preserve qualified definitions and validate digests and closure. Adding a Decider is an extension of that contract, not a separate latest-configuration lookup.
- The existing Jev request protocol uses a question map for transport correlation. TypeSafe's [official question contract](https://docs.typesafe.ai/primitives) and [HTTP API contract](https://docs.typesafe.ai/api), checked on 2026-10-08, explicitly state that question keys do not reach the underlying model. Aidash must put the complete rubric in instructions and avoid copying internal question IDs into those instructions or the state.

Round 3 settles remote placement, the new policy shape, state-disclosure fields, and replay-format details. The final shared-understanding confirmation and implementation request were received on 2026-10-08.

## Round 3 frontier

Q7-Q16 unblocked six remaining boundary decisions. Q17-Q21 were accepted as recommended; Q22 was changed to remove Aidash fixed limits. The Q22 correction takes precedence over earlier size/cardinality-bound wording.

| Question | Decision | Accepted decision | Status |
| --- | --- | --- | --- |
| Q17 | Remote placement and owner reservations | The Execution node builds the state, evaluates the gate, and dispatches its pinned Decider. Home must admit that exact qualified Decider/configuration and disclosure scope; credentials remain on Execution. No Home or environment-provider fallback and no third-node dispatch are introduced. Each allowance owner commits atomically within its own transaction; dispatch waits for all durable owner receipts and a durable dispatch record. Only a proven pre-dispatch abort may release a provisional reservation; after dispatch, uncertainty stays charged. Canonical evidence stays in the executing Run journal; Home receives authorized receipts/summaries rather than automatic full-state copies. | Accepted |
| Q18 | New policy and permission shape | Agent Bindings select the Decider; Execution policy gives each exact hook/Decider a max_calls_per_run cap, while generation policy uses decisions keyed by hook with the same qualified Decider and calls_per_agent/call_budget. Remote allowances use this same qualified shape, without duplicate local/remote compaction fields. Effective caps are the intersection of pinned allowances and current stricter authority, and durable counters never reset on recovery. Use decision.invoke, decision.evidence.read, and decision.state.read with exact resource/hook scopes; legacy compaction fields and actions are rejected for new registrations, while historical records remain readable. | Accepted |
| Q19 | Compaction disclosure allowlist | Allow only the task goal, disclosure-permitted conversation text, fixed Compaction rubric, authorized/redacted tool names and input views, and result status/length notes. Do not copy full Agent/Skill instructions, private reference or file bodies, raw tool outputs, or credentials into decision state. Inherited source restrictions also govern derived text; events with forbidden or unknown disclosure provenance are retained and excluded from probabilistic pruning, rather than treated as irrelevant. Complete inference fitting still counts the original private context. | Accepted |
| Q20 | Exact external model pin | Require a concrete published model version, reject mutable aliases such as jev-latest/jev-preview, and validate the returned model version before accepting answers. Keep actual external-provider capabilities and provider contract versions in the reviewed immutable dependency snapshot; Q22 removes Aidash fixed request/response ceilings. A missing or different response model is a provider-contract failure; switching models requires a new reviewed Decider version and Run. | Accepted |
| Q21 | Historical replay and numerical precision | Version the evidence format and pure branch evaluator, retain exact validated binary64 probabilities/thresholds, and use the original comparison semantics including equality at 0.5. Branch replay is read-only and uses historical rules; current reader/source authority still controls access. Recovery or applying an action requires current execution authority and restrictions. Unsupported/corrupt evidence fails explicitly, and diagnostic provider reruns produce separate charged attempts. Display rounding never changes evaluation. | Accepted |
| Q22 | No fixed limits and observed outcomes | Aidash imposes no fixed maximum state/request/response/evidence size, total question count, batch count, or parallel request count. External-provider capabilities, explicit call allowances, current authority, and storage/runtime failures still govern execution; none becomes a hidden substitute for the rejected fixed ceilings. Build and validate the complete candidate/question set, split only as required by actual provider constraints, and retain complete protected evidence without truncating it to an Aidash size cap. Failures preserve the original context. Outcomes record applied/shadow/rejected/uncertain, fitting estimates and retained/dropped/truncated counts, and separately link the next inference; they make no causal-success or model-quality claim. | Accepted with correction |

### Facts shaping Round 3

- [Remote compaction admission](../../crates/aidash-application/src/generation/protocol.rs) requires the dispatcher to be the Execution node and pins provider metadata/configuration, even though the surrounding remote semantic contract is named RequiredHome. There is no need to add a Home-dispatched Decider to preserve this placement.
- [Remote allowance operations](../generation.md) reserve each owner's budget before HTTP and retain durable dispatch/outbox records. A distributed reservation is not one database transaction across Nodes; all owner receipts are required before dispatch.
- TypeSafe's [model contract](https://docs.typesafe.ai/models), checked on 2026-10-08, distinguishes concrete model IDs from mutable aliases and reports the concrete responding model. Pinning the Registry definition alone is insufficient if its model name is an alias.
- [Noul's documented meaning](https://docs.typesafe.ai/primitives) is the probability of yes, without a separate confidence field. The existing Compaction threshold is a keep heuristic; it is not evidence that external-write approval is calibrated or safe.

The final design and acceptance checklist below close every branch of the decision tree. The user confirmed the complete design and requested implementation. PR publication was subsequently requested; the native integration remains incomplete.

## Final design

### Contract and responsibility

The Registry declares an immutable Decider with its executable hook, exact probability-provider/model configuration, versioned Node state builder and option source, complete question rubrics, thresholds, and mode. The first supported hook is Compaction and its first supported answer type is Noul. Registration rejects empty descriptions, unsupported hooks/question types, unknown builder/option-source contracts, mutable model aliases, duplicate hook Bindings, and invalid restrictions. Other hooks and Choice/Score execution require later supported implementations; model routing and external-write automation are outside #108.

Pure decision values and branch rules belong to `aidash-domain`. `aidash-application` owns DecisionGate and the authority, allowance, and evidence ports. `aidash-harness` calls the gate at the fixed Compaction boundary. `aidash-integrations` supplies the Jev adapter, and `server` composes HTTP and persistence adapters. This preserves the current portable dependency direction.

Each Run resolves one exact Decider per bound hook. The snapshot retains its qualified definition identity/digest, effective configuration, provider/builder contracts, concrete model version, restrictions, and mode. Current policy can restrict the snapshot further but cannot introduce a different provider, model, or wider action. Credentials are resolved locally at invocation and never copied into evidence or remote metadata.

### Execution sequence

1. Compute the complete inference-request fit in code. If it already fits, perform no probability-provider request and consume no Decision allowance.
2. Check mandatory authority and rules. Preserve the first event, recent six events, and events whose content cannot be disclosed or whose provenance is unknown. The inference fit includes original private context even though the classification state omits it.
3. Build only permitted Compaction state and options. Carry task goal, permitted conversation text, fixed rubric, redacted tool input views, and result status/length notes; exclude full Agent/Skill instructions, private document/file bodies, raw tool results, and credentials. Each candidate has an exact revision-scoped identity and complete description. Question IDs serve transport correlation only.
4. Validate all questions and plan requests against actual capabilities of the pinned external provider. Split independent questions when those actual constraints require it, without Aidash fixed size, question, batch, or concurrency ceilings. No candidate is silently omitted to satisfy an internal ceiling.
5. Before each physical request, recheck catalog/invocation/disclosure authority and obtain durable call reservations and a dispatch record. Generated local ancestors reserve atomically within their owner; cross-Node dispatch waits for every owner's receipt. Every batch and retry has its own attempt. Proven pre-dispatch abort may release its provisional reservation; dispatched, failed, crashed, and uncertain attempts remain charged.
6. Require the exact responding model, complete expected question coverage, correct Noul types, and finite probabilities in [0,1]. Do not accept a partial answer set or apply successful batches independently.
7. Evaluate the immutable keep threshold, initially 0.5, with its inclusive comparison. Keep the entire event if keep-result meets the threshold; otherwise keep the call and truncate its result if keep-call meets the threshold; otherwise remove the paired event from reasoning history. Additional restrictions may only preserve more content or forbid application. The execution journal retains original effects and outputs.
8. Apply Enforce only if the complete candidate inference request fits and current worker/revision/input/source authority still permits it. Commit context and application evidence atomically. Shadow records the proposed branch without changing the context. Other failures preserve the original context and surface the fitting/decision failure.
9. Append later outcomes linked to the decision, including the next inference reference and observed status. Record measurable fit and retention changes without labeling downstream success as caused by the decision.

### Allowance and permission shape

| Owner | Logical field | Meaning |
| --- | --- | --- |
| Agent definition | Decider Binding | Exact qualified declaration chosen for one hook. |
| Execution policy | `decisions[hook].decider` | Exact Decider identity and reviewed definition/configuration digests. |
| Execution policy | `decisions[hook].max_calls_per_run` | Explicit provider-attempt allowance for that Run; absence grants no calls. |
| Generation policy | `decisions[hook].decider` | The same qualified Decider/digest shape used for local and remote approval. |
| Generation policy | `decisions[hook].calls_per_agent` | Provider-attempt allowance allocated to one generated Agent. |
| Generation policy | `decisions[hook].call_budget` | Shared lifetime allowance owned by that generation policy. |
| Current authority | `decision.invoke` | Invocation permission scoped to the exact resource/hook and permitted disclosure. |
| Current authority | `decision.evidence.read` | Detailed evidence permission, additionally requiring Run and applicable source access. |
| Current authority | `decision.state.read` | Full retained-state permission, additionally requiring evidence/source access. |

Allowances are explicit owner-selected budgets rather than Aidash universal ceilings. The effective remaining allowance is the intersection of the pinned Run allowance, generated ancestor allowances, and current stricter authority. Restart, retry, and policy widening cannot reset or replenish consumed allowance. Full-state recording remains disabled by default; it requires tenant-authorized Node policy before evaluation, defaults to seven days, and cannot exceed thirty days or a stricter source retention period.

### Evidence and replay

The evidence record contains a versioned identity, Run/step/input revision, hook, exact Decider/provider/builder/rule identities and digests, actual sent-state digest, request digest, candidate identities/descriptions, validated answers, effective restrictions, mode, chosen branch, reason, and attempt/outcome references. Numeric probabilities and thresholds use a lossless binary64 representation for evaluation; UI rounding is only display formatting. Mandatory field redaction precedes disclosure and digest calculation. No raw provider error/debug body or credential is retained.

Retain complete authorized evidence; Q22 rejects a fixed evidence-size ceiling and size-based truncation. Storage or runtime failure prevents context application and follows existing recovery rather than pretending an incomplete evidence record is complete. Full state, when opted in, is the exact permitted state sent to the provider; expiry leaves an explicit expired-state reference. Detailed evidence follows existing journal retention and remains subject to current source/reader authority. Ordinary Run events contain a safe summary and a click-through evidence reference.

Branch replay is read-only and uses historical validated answers, thresholds, and evaluator versions. An unknown evidence/evaluator version or corrupt record fails explicitly. It does not substitute current policy or call a live provider. Recovery can reuse durably recorded answers only for the same valid decision input and current permitted restrictions; actual action always rechecks live authority. Provider reruns are separately authorized and charged diagnostic attempts.

### Remote placement and cutover

Execution builds and dispatches the decision; Home approves that exact Execution-node dependency and disclosure scope. All applicable generation ancestors agree on the same qualified Decider and reviewed configuration. Home receives only authorized receipts and summaries, while Execution retains credentials and canonical journal evidence. There is no provider substitution on timeout, revocation, or unavailability.

Deliver new registration/policy contracts, bilingual existing administration forms, and Run-journal summary/detail views against the native Binding path after PR #133 integration. Drain old work before deployment. New registration and execution reject `compactor`, environment-only selection, legacy compaction policy fields, and old invocation grants; owners explicitly register and approve Deciders. Retain historical definitions and journals for read-only inspection without a migration or executable compatibility projection.

## Acceptance checklist

These are implementation requirements, not completed runtime test results.

- [ ] Registration accepts supported Compaction/Noul Deciders and rejects empty descriptions, unknown contracts, unsupported hooks/types, mutable aliases, duplicate hook Bindings, and restrictions that permit more deletion.
- [ ] A Run's exact dependency snapshot remains stable through later definition/installation changes; current revocation or tighter policy still prevents forbidden requests or actions.
- [ ] Compaction preserves current keep/truncate/drop behavior and protected history; already-fitting contexts issue no provider request. Shadow never changes context.
- [ ] Missing approval, unavailable provider, wrong/missing response model, malformed/incomplete answers, and insufficient Compaction preserve the original context and never grant approval.
- [ ] Every physical HTTP attempt has a durable reservation and identity; concurrent batches cannot overspend explicit owner allowances. Unknown attempts remain charged and retries never reuse a dispatched identity.
- [ ] Cross-Node dispatch waits for all owner receipts, handles partial pre-dispatch failure and receipt replay, and validates exact Execution-node dependencies and Home disclosure authority.
- [ ] Worker lease loss, Run/input revision changes, source revocation, or evidence persistence failure prevent application. Applied context and evidence commit atomically, including crash recovery.
- [ ] Disclosure fixtures prove exclusion of full instructions/private material/credentials and retention of undisclosable or unknown-provenance events; detailed evidence and full state enforce distinct permissions.
- [ ] Full-state recording requires prior opt-in, honors seven-day default/thirty-day ceiling and source restrictions, and exposes expired references after deletion.
- [ ] Replay reproduces the historical branch with exact numerical boundary cases, performs no provider I/O, and fails unsupported or corrupt records explicitly.
- [ ] A capable fixture provider can exceed every rejected fixed size/count/concurrency value without an Aidash ceiling or evidence truncation. Real provider constraints and explicit call allowances still apply.
- [ ] New-schema API and English/Japanese administration/journal views cover binding, allowance, approval, evidence, and state access. Historical inspection works without executable legacy compatibility.
- [ ] Relevant existing Compaction tests are ported to explicit Decider fixtures; local ordinary/generated/remote and crash/revocation acceptance pass on the integrated native Binding path.

## Decision-tree closeout

| Original branch | Settled by |
| --- | --- |
| Scope, executable hooks, failure, modes, activation | Q1, Q7, Q12, Q16 |
| Provider coupling, builders, options, batching, disclosure | Q2, Q9, Q12, Q19, Q20, Q22 |
| Threshold ownership, narrowing, comparison boundaries | Q3, Q11, Q21 |
| Compatibility, Binding/policy shape, budgets, remote pins, cutover | Q4, Q8, Q10, Q16, Q17, Q18 |
| Evidence fields, permissions, state retention, outcome linkage | Q5, Q13, Q14, Q15, Q19, Q22 |
| Replay meaning, identity, recovery, atomic persistence, validation | Q6, Q15, Q21, acceptance checklist |

Every design branch is settled. Q22 supersedes earlier fixed size/cardinality/concurrency-bound proposals; it does not reopen approved call budgets, retention durations, authority, or actual external-provider capabilities. Implementation status and remaining native integration prerequisites are tracked in [implementation status](2026-10-08-issue-108-implementation.md).
