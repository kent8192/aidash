# Local Issue #103 and #108 amendment drafts

Status: design confirmed; issue-body amendment drafts included for PR review. The referenced GitHub Issue bodies have not been edited. The [design ledger](2026-10-08-issue-108-decision-layer.md) records the accepted decisions and the implementation acceptance checklist.

## Issue #103: decision-layer decisions

Replace the open threshold-location and decision-log-state proposals with the following settled decisions:

- **Threshold location:** Mandatory authorization and safety rules live in operator code. Per-action thresholds belong to the immutable Decider version. Agent and Node restrictions may only narrow permitted behavior; evidence retains the exact effective rules for replay.
- **Decision-log state:** Retain the actual sent-state digest and access-controlled decision evidence by default. Full state requires explicit tenant-authorized Node policy before evaluation, defaults to seven days, and has a thirty-day ceiling subject to stricter source restrictions. Detailed evidence and full state have distinct read permissions in addition to Run/source authority. Mandatory redaction precedes disclosure and hashing.
- **Replay:** Reproduce the branch from recorded validated answers and historical rules. Live provider reruns are separately authorized, charged diagnostic attempts and do not guarantee equal probabilities.
- **Decision limits:** Aidash introduces no fixed ceilings for state/request/response/evidence sizes, total questions, batch count, or request concurrency. Actual external-provider constraints, explicit owner-approved call allowances, authority, and retention rules remain applicable.

For Step 2, distinguish the versioned Decider declaration, its probability provider, and the operator DecisionGate. The first executable hook/question type is Compaction/Noul. Choice/Score and other hooks require later supported implementations; external-write automation belongs to #109 and model routing remains out of scope.

Issue #108 replaces legacy compactor registration and policy selection through an explicit breaking cutover. Do not describe a compactor projection or migration as part of this step. Other #103 steps and decisions remain outside this amendment.

## Issue #108: replacement body

### Summary

Step 2 of #103 introduces immutable `decider` Registry declarations, a probability-provider-independent `DecisionGate` application port, and protected Run-journal evidence. The first executable decision is existing Jev context Compaction using Noul questions. The implementation depends on the native Binding execution path from #106 and PR #133.

### Declared dependencies

An Agent explicitly binds at most one Decider per executable hook. The Run snapshot pins its qualified definition/digest, provider/builder/option-source contracts, effective model configuration, concrete external model version, mode, and restrictions. Registration rejects empty descriptions, unknown Node contracts, unsupported hooks/question types, duplicate hook Bindings, mutable model aliases, and widening restrictions.

State builders compute counts, sizes, fitting, and comparisons in code. Compaction uses an explicit disclosure allowlist: task goal, permitted conversation text, fixed rubric, redacted tool input views, and result status/length notes. It excludes full Agent/Skill instructions, private document/file bodies, raw tool outputs, and credentials. Undisclosable or provenance-unknown events remain retained and are not probabilistic pruning targets. Candidates have complete descriptions and stable revision-scoped identities; question keys provide transport correlation rather than model instructions.

### Operation

The Execution node calls DecisionGate at the fixed Compaction boundary. If the complete inference context already fits, it performs no provider call. Otherwise it checks hard rules and current authority, builds the permitted state/options, groups independent questions for the same exact request scope, and reserves each physical attempt before provider I/O. Remote work requires Home approval of the exact Execution-node dependency and disclosure, plus durable allowance receipts from every applicable owner before dispatch.

All expected answers and the responding model must validate before branch evaluation. Preserve the existing 0.5 keep threshold, first event/recent six protections, and keep/truncate/drop ordering. Enforce applies only a completely fitting candidate, with context and evidence committed atomically under worker/revision fencing; Shadow only records the proposed branch. Invalid, unavailable, unapproved, stale, or insufficient decisions preserve original context and never grant automatic approval. Failed or uncertain dispatched attempts remain charged; a retry is a new attempt.

### Policy and evidence

Use `decisions` keyed by hook with exact qualified Decider/digest approvals. Execution policy supplies `max_calls_per_run`; generation policy supplies `calls_per_agent` and `call_budget`. Local and remote approvals use the same qualified shape, and current stricter authority intersects with the pinned allowance. Add exact-scope `decision.invoke`, `decision.evidence.read`, and `decision.state.read` authority. Recovery cannot reset counters.

Evidence retains exact declaration/builder/rule identities, sent-state/request digests, candidates/descriptions, validated answers, restrictions, mode, branch, reason, attempts, and appended outcomes. Historical branch replay is read-only and uses exact numeric values and historical evaluator semantics. Ordinary events expose safe summaries; detailed evidence requires Run/source and dedicated evidence authority. Full state is opt-in with a seven-day default/thirty-day ceiling and stricter source restrictions.

There are no Aidash fixed size, question, batch, or concurrency ceilings. Actual provider capabilities determine necessary request splitting. Preserve complete authorized evidence; do not truncate it to an internal byte cap or omit candidates to satisfy an internal count cap. Provider, storage, and runtime failures preserve original context and use the durable recovery protocol.

### Acceptance criteria

- [ ] Compaction executes through DecisionGate with the existing protected-history and keep/truncate/drop semantics; relevant tests use new explicit Decider fixtures.
- [ ] Registration, immutable Run Binding resolution, generation/remote approvals, and English/Japanese administration forms support the new contract.
- [ ] Wrong/missing model pins, missing permission, provider unavailability, malformed/incomplete answers, revision/lease conflicts, and persistence failures never apply partial context or grant approval.
- [ ] Each HTTP attempt has durable identity/accounting; ordinary/generated/remote budgets, receipt replay, partial pre-dispatch abort, and uncertain retry behavior are verified.
- [ ] Compaction disclosure, detailed-evidence/full-state permissions, prior opt-in, source revocation, retention expiry, and safe journal summaries are verified.
- [ ] Historical evidence reproduces the branch without provider I/O, including threshold equality and corrupt/unsupported-record cases.
- [ ] A capable fixture provider exceeds the rejected fixed size/count/concurrency values without Aidash rejection or evidence truncation; actual provider constraints and explicit call budgets still apply.
- [ ] Crash recovery preserves context/evidence atomicity and worker/input fencing.
- [ ] New contracts reject legacy compactor/configuration/grant shapes; historical journals remain inspectable.
- [ ] The threshold, state-retention, replay, and no-fixed-limits decisions are reflected in #103.

### Compatibility and cutover

This is a breaking change. Register and approve new Deciders explicitly; no legacy compactor projection, environment fallback, policy migration, or executable compatibility layer is provided. Drain existing work before deployment and retain historical definitions, receipts, and journals. No new provider or model is selected automatically on failure.

### Out of scope

External-write automation/calibration (#109), model routing, other executable hooks, and Choice/Score execution are deferred to their own supported contracts.
