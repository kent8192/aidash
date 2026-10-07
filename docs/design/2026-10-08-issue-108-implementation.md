# Issue #108 implementation status

Design confirmed and implementation requested on 2026-10-08. PR publication was
subsequently requested. The review branch is `feat/issue-108-decision-layer` in
the reused design worktree; this checkpoint is submitted as a Draft PR.
The complete [accepted design](2026-10-08-issue-108-decision-layer.md) remains the
delivery contract. This checkpoint implements its portable foundation; Issue
#108 is not complete and the new Gate is not connected to production execution.

## Portable foundation

- `aidash-domain::decision` supplies strict Compaction/Noul declarations, concrete
  Jev model pins, immutable contract identities, narrowing, exact ordinary and
  generated allowance types, opt-in state retention, candidate/attempt/outcome
  evidence, safe summaries and deterministic historical replay.
- Probabilities and thresholds serialize as 16 lowercase hexadecimal binary64
  digits (`0.5` is `3fe0000000000000`). This avoids dependence on JSON decimal
  parsing during replay. Digests use the existing `sha256:<hex>` convention.
- Explicit Decider Bindings retain definition/configuration/provider/builder
  pins, permit only restrictions that preserve more history, and reject duplicate
  hooks, foreign execution placement, missing Node support and altered snapshots.
  A Node without a composed Decision Provider fails admission explicitly.
- `aidash-application::decision::DecisionGate` constructs an authorized view,
  preserves unknown/forbidden history, protects the first/recent events, requires
  live authority and durable owner receipts for each physical request, persists
  complete validated answers, and requires a fenced atomic journal/context commit
  before changing caller context. Shadow only records a proposal. An already-fitting
  request needs no provider, approval or allowance.
- The Node-owned state builder accepts only authorized goal/conversation/tool-input
  views and computed result status/length. It never reads Agent/Skill instructions,
  pinned private documents, summaries, raw tool arguments or raw result bodies.
  Full original inference input is counted when fitting and identified by its digest.
- `aidash-integrations::decision::JevDecisionProvider` sends exact prepared bytes,
  requires the concrete response model and complete valid Noul answers, redacts
  transport/provider errors, and disables automatic retries and redirects. Request
  planning uses an injected factual external-capacity catalog. No Aidash byte,
  question, batch or concurrency ceiling is added. Provider-capacity/tokenizer
  implementations must be supplied by the native composition; an unbounded fixture
  capability is used only by tests.

The journal and authority interfaces are mandatory ports, not database
implementations. Fixture transactions demonstrate the orchestration contract;
they do not certify native durability, generated ancestry accounting or federation.
The existing compactor runtime is still present until the breaking native cutover
can be completed; it is not used as an adapter or fallback for the new Gate.

## Native prerequisite

The baseline is `develop/0.1.0@015a8a4e9cb95ebf4ad93c1c279015c76bac7f48`, including
the native Hindsight memory replacement. [PR #133](https://github.com/kent8192/aidash/pull/133)
was verified open and conflicting at `b3950f5b9cc96fb774e5fbc1721d27cbc63a8ce4`.
A local no-commit merge was inspected and aborted. Its Binding cutover still
derives old conversation/semantic memory adapters while current develop uses
native memory providers and sources. The conflicts include Agent settings,
Registry validation, authorization, harness source assembly, migrations and
native acceptance fixtures. Taking either side wholesale would discard one
accepted behavior. No dependency commits or conflict markers remain here.

Before production activation, reconcile that native Binding cutover with the
current memory model, preserving both exact capability admission and native
memory authority/recovery. Then complete:

1. Compose the Gate at the fixed harness Compaction boundary, with a versioned
   source/provenance/disclosure adapter and actual Jev capacity/tokenizer catalog.
2. Wire the accepted `decisions[hook]` policy shape through ordinary execution,
   generated ancestry and remote Home/Execution admission. Implement durable
   reservations, current/pinned cap intersections, remote owner receipts,
   uncertainty and replay without budget resets or provider substitution.
3. Add owning-app persistence and additive migrations using SeaORM/SeaQuery per
   the user-provided repository instruction. Implement atomic evidence/context
   commits, worker/input fencing, durable answer recovery, outcome links and
   expiration markers. Retain historical records without executable migration.
4. Add permission-enforced evidence/full-state APIs and English/Japanese existing
   admin/journal views. Add the database Registry kind and generated API contracts
   together; the portable `decider` contract alone does not make it available in
   the current native Registry API.
5. Remove the legacy compactor/environment selection and old compaction policy/
   invocation grants from the new execution contract. Drain old work and require
   fresh registrations and approval; retain read-only historical journals.
6. Verify ordinary/generated/remote native execution, concurrent budgets, crashes,
   fencing, revocation, state expiry, historical readers and bilingual UI behavior.

## Verification

Completed portable checks (latest results for each package):

- `cargo test --locked -p aidash-domain -p aidash-application -p aidash-integrations`
  passed: 864 Domain unit tests, eight policy integration tests, 2,906 Application
  unit tests, six authorization integration tests and 93 Integrations tests
  (3,877 total). The Application suite was rerun after adding a multi-worker
  concurrency fixture, which also proves the Gate future can run on worker threads.
- Focused HTTP fixtures prove exact model/answer coverage, lossless near-threshold
  decimal conversion, duplicate-key rejection, payloads/responses over 1 MiB and
  no unreserved redirect. Capacity splitting preserves every question.
- Gate fixtures include 106 provider-fixture batches without a fixed fanout cap, strict owner
  receipts, budget contention, protected provenance, wrong pins/input revisions,
  Shadow, revocation, invalid/incomplete answers and persistence/lease failures.
- `cargo fmt --all --check`, `git diff --check`, production workspace-boundary
  validation and its three Python tests passed.

`cargo clippy --locked -p aidash-domain -p aidash-application -p aidash-integrations
--all-targets -- -D warnings` passed; Application Clippy passed again after the
additional worker fixture. Native server/database/browser acceptance has not run
because the Gate has not been composed into that runtime. No live
TypeSafe request or GitHub Issue body edit has been made. Draft PR publication
does not mark the native integration or Issue #108 acceptance checklist complete.
