# Decision gate

The `DecisionGate` makes explicit, version-pinned Compaction decisions for #108.
Native [context recovery](protocol.md#context-recovery) runs its pruning stage
through `aidash_application::context::compact_decided`, which calls the gate with
the Run's compaction Decider and the Node's authority, journal and provider
adapters. A Run without a compaction Decider, or with one in Shadow mode, is not
pruned; its Summary Stage may still run on its own. A bound Decider that cannot
run pauses the Run with `decider_unapproved`, `decider_calls_exhausted` or
`decider_unavailable`, and the Summary Stage, a separately approved generation
step, never stands in for it.

Decider declarations name `provider_credential: "openrouter"` and list one to
four Decider Routes in `routes`, tried in order (ADR 0066). A System One Route
(`provider_contract: "typesafe.jev/1"`) requires an exact OpenRouter Jev release
such as `typesafe/jev-1.14` (aliases are refused) and names no hosts. A Decisions
Route (`"openrouter.decisions/1"`) requires an exact model ID such as
`cloudflare/clef` (no `~` alias, no `:` variant), one to eight host slugs in
`hosts`, and no long-context mode. Each Route has its own `threshold`
(above 0 and at most 0.5) and `state_budget`: the bytes of state and request
its default mode takes, the long-context limits (System One only), and the
tool-input abridging stages. A version also declares its per-Run call limit
(`max_calls_per_run`, shared by every Route and lowered only by a Binding) and
whether it keeps the classification state (`state_retention`, allowed by the
version's Catalog Approval). The builder `aidash.compaction/3` abridges
disclosed tool inputs stage by stage until the state fits the Route's default
mode, sends only the most abridged state in the long-context mode, and treats a
larger one as that Route being unavailable. On the `native` Projection Version
it protects whole Model Turns. Its rubric travels first in the state, under the
key `brief`, and says what each answer does: a kept call keeps a cut result's
first 300 characters, and a dropped call leaves the agent unaware it ran (ADR
0107). They configure no endpoint or key: the adapter
calls OpenRouter's System One API, or its Decisions API with `provider.only` set
to the Route's hosts, with `provider.zdr = true`, resolving the Tenant's current
OpenRouter Provider Credential for each dispatch rather than when a request is
prepared, so a revoked or unbound credential stops later attempts (ADR 0041). A
response must name the pinned model or a dated snapshot of it, and a Decisions
response must name one of the Route's hosts. The adapter validates the
declaration before preparing requests.

A later Route is asked only when the Route before it was unavailable: a
connection failure, a timeout, HTTP 404, 429 or 5xx, an answer that is malformed
or fails validation, or a state or plan the Route does not accept. HTTP 400,
401, 402, 403 and 413, a missing binding, a revoked credential, authority,
journal and recovery failures stop the decision on the Route that saw them. A
Route is never chosen for its answer. Each Route is a decision of its own: its
identity commits to the Route's place, and its rejected evidence stays when the
next Route answers. The Binding restrictions are intersected with the Route's
threshold, so a Binding's stricter threshold still applies. When every Route
was unavailable for a provider reason, the step returns `DeciderOutage` and is
retried after 2, 4, 8, 16 and 32 seconds; each retry is a decision of its own
through `Boundary::retry` (`RecoveryState::decider_retry`). After the fifth the
Run pauses with `decider_unavailable` until a resume. A state no Route accepts
pauses at once.

`DeciderConfig::digest()` commits to the canonical JSON object containing
`models` (the Route models in order), `thresholds` (their thresholds),
`mode` and `parameters_digest`. The latter hashes the complete serialized
configuration with every Route `model` and `threshold` and the `mode`
replaced by JSON `null`, so evidence that
sent no request still proves the mode its Decider ran in. Historical `Evidence` retains that hash as
`configuration_parameters_digest` with `route_models` and `route_thresholds`,
allowing replay to reconstruct and check the configuration pin; `route` names
the Route asked, and `model` and `provider_contract` are that Route's. Replay
refuses `restrictions` whose threshold is looser than that Route's, so evidence
cannot claim a more destructive result than the pinned Route permits. A Route
that refuses the state, because it does not fit the Route's budget, or a plan
records `provider_failure` evidence with no attempt before the next Route is
asked. Credentials are never copied into
evidence. No current definition, policy or provider call is
needed to verify the commitment.

Replay checks dropped counts against the recorded branches and requires the
truncated count to be no greater than the number of `TruncateResult` branches.
Those branches can leave short results unchanged, so their count is an upper
bound rather than an exact truncation count. The complete candidate must fit
before Enforce can apply it; Shadow records the proposal without applying it.

A compaction Decider version names its rule. `compaction.keep/1` reads each
candidate against the threshold alone. The opt-in `compaction.fit/1` (ADR 0105,
#611) starts from those branches. While the candidate does not fit, it turns
still-kept, unprotected candidates whose keep-result probability is below 0.7
into `TruncateResult`, lowest probability first and older history first on ties.
Replay recomputes the threshold branches and then requires the truncated ones to
be exactly the first ones in that order.

Rust consumers must include the required `Evidence::configuration_parameters_digest`
field and obtain configuration pins through `DeciderConfig::digest()`. Earlier
development revisions that hashed the configuration directly must reconstruct
their pins from the approved definition; replay does not substitute a model or
default a missing commitment. There is no legacy compactor projection: the `compactor` kind was removed (#108).

Execution receives the `BoundDecider` reconstructed by `BindingSnapshot::decider`,
keeping the exact pin, approved configuration and immutable Binding restrictions
in one value. The Gate validates that configuration against the pinned definition.
Classification questions use those admitted restrictions so recovery has the same
request plan; live restrictions can only preserve more of the proposed history.

Every fresh request rechecks source/invocation authority and the source deadline
after its durable reservation has returned, immediately before dispatch. A revoked
or expired reservation stays charged and is finalized as `NotDispatched`; no
provider I/O occurs. Valid live `forbid_apply` restrictions remain in rejection
evidence. Authority and journal failures retain their original application error
category and use `AuthorityFailure`/`JournalFailure` reasons rather than blaming
the provider. Invalid answers and actual transport failures remain distinct.

Decision identities commit to the Node, Run, step, the Run's resume cycle, the
step's outage retry, input revision/digest, exact Decider pin, the Route,
Binding restrictions, classification state, sources and target window. The resume cycle grows each time a person resumes a
paused Run, so a resume starts a new decision, while a recovered lease continues
the same one. A recovered attempt whose result is unknown is never dispatched
again: the Run pauses with `decider_unavailable` until a resume.
Attempt identities additionally commit to exact request bytes and question IDs.
Worker tokens and mutable Run revisions are excluded from identity and checked by
the journal as live fences. `DecisionJournal::recover` loads every charged attempt
before planning requests. Only validated durable answered attempts are reused;
pending/uncertain attempts return a conflict without another charge or request.
Recovery also preserves existing attempts when current authority is revoked.
`reserve` must atomically return `Reservation::Recovered` if another worker reserved
the same attempt after the recovery read; overlapping or changed plans are rejected.
Applied/Shadow evidence cannot be overwritten on retry. Rejected snapshots remain
immutable and may be resumed using the same charged attempts, so a transient journal
failure keeps its retry semantics. A later attempt appends evidence rather than
rewriting earlier snapshots. These are mandatory portable adapter contracts, not
claims of native PostgreSQL crash acceptance.

Evidence version 3 requires an explicit `state_retention` witness. Retained and
expired markers must match its immutable state ID and deadline and the existing
state digest. Cleanup can remove bytes and replace a retained marker with its
matching expired marker, but cannot erase the witness or mark retained state as
`Disabled`. Earlier evidence versions are rejected rather than defaulting a witness.
Version 3 also keeps the Binding restrictions admitted with the Run, which the
decision identity commits to, so a decision without attempts verifies its identity;
each attempt's commitment to the definitions of the questions it asked; the Model
Turn of each candidate and the number of turns on `native`; the history entries a
pruning removed; and, for an applied or staged decision, the digest of the
projection it produced. `incomplete_answers` separates durable answers that do not
cover the questions from `invalid_answers`, which needs the failed attempt that
returned them. A decision whose pruning was not enough is handed back uncommitted
as `CompactionResult::Insufficient`: the Summary Stage continues from its pruned
projection, and the decision is recorded as `staged` in the transaction that adopts
the summary, or as rejected when none is adopted (ADR 0056). A decision with nothing
to classify returns `CompactionResult::Unpruned`.

Rust adapters must update `Evaluation::decider` to accept `&BoundDecider`, include
`DispatchRecord::binding_restrictions` and `DispatchRecord::question_digest`, key each
attempt by `attempt_identity`, implement `DecisionJournal::recover`, and return
`Reservation` from `reserve`. Handle `AttemptStatus::NotDispatched`, the failure
reasons and the required `Evidence::state_retention` field.

## Approval hook

The approval Decider judges one pending Tool call of a Run for the Auto Approval Mode (ADR 0077). It is a Decider of hook `approval`. It uses the same gate as compaction, but it never pauses a Run for its own sake. The Tenant chooses its version. Until the Tenant names another `decider` of hook `approval`, the version is the built-in `aidash.approval-decider@1.0.0`, which Aidash seeds into every Tenant Registry. Auto needs a Catalog Approval of the exact version on the Node. The routes that read and set the choice are in [Approval Rules and Modes](authorization.md#approval-rules-and-modes).

The built-in version runs in `enforce` mode with `max_calls_per_run` of 200. It tries two Routes in order. `typesafe/jev-1.14` runs through System One. `cloudflare/clef` runs through OpenRouter's Decisions API on `cloudflare`, `deepinfra` and `primeintellect`. Both Routes block at `threshold` 0.3, which is not yet calibrated for approvals. A changed threshold is a new version. Approving the version approves sending a Run's state to each Route.

The Decider asks five questions about the call, one for each block reason: `data_exfiltration`, `destructive`, `beyond_request`, `injected` and `boundary`. A category blocks the call when its probability reaches the effective `threshold`. The most probable blocking category names the reason, and on a tie the category listed first wins. A call that no category reaches is allowed. A lower `threshold` is stricter. An Agent's Binding can lower a Route's threshold, but it never raises one.

The state holds the inputs people sent to the Run's Thread, the Agent's instructions, the Run's earlier tool calls without their results, and the pending call with its tool's definition. It never holds tool results or the model's own text. Only inputs whose author is an enabled user subject of the Run's Tenant count as people's inputs, so a message another Node's agent sent never does. Text a tool returned reaches the Decider only where the model copied it into a call's arguments.

An allowed call runs at once, without another model response. A blocked call is refused, and the model receives `tool call was blocked by Auto: <reason>` as its error. The reason is one of the five block reasons, or `unavailable` when no Route answered. Unlike compaction, an unavailable approval Decider never pauses the Run. A version in `shadow` mode records its verdict and settles nothing, so the person is asked as in Manual. A spent `max_calls_per_run` allowance records no decision and leaves the call to a person.

Each decision records an `ApprovalEvidence`. It holds the `verdict`, the `answers`, the Route that answered (`route`), the Routes that were unavailable before it (`failed_routes`), the effective `restrictions` and the `binding_restrictions`, the `mode`, and the `attempts`. It keeps the digest of the state, not the state. A verdict from the Route asked can be checked again from its answers and the effective threshold.

## Native persistence

`RunDecisionJournal` (`server/src/apps/execution/repositories/decisions.rs`) is the
PostgreSQL `DecisionJournal` for one leased Run and the exact Decider pin admitted
for it. Its boundary maps `run_revision` to `runs.revision` and `input_revision` to
`runs.observed_input_seq`; reserving, recovering and committing lock the Run row and
require the current lease and both revisions, and the boundary's step and revisions
must also match the Run snapshot the journal was created with. A Decision Attempt is charged once, as a
`pending` row keyed by its stable identity, together with a receipt for the Run's own
per-Run call limit (`ExecutionAllowance::max_calls_per_run`). Every reservation reads
the Run's current policy again through `CurrentAllowance` inside its transaction and
charges against the stricter of the admitted and current limits. Reserving the same
attempt again returns it as recovered. A changed charge, an overlapping question plan
or another batch of the decision with different inputs (input commitment, state digest,
mode or binding restrictions) is a conflict, a current policy that pins another Decider
is invalid, a spent limit stops with `decider_calls_exhausted` and a Decider the current
policy no longer allows stops with `decider_unapproved`, before anything is charged. An
attempt that is not keyed by its stable identity, or whose Binding restrictions fall below
the floor every Decider shares, is refused. Recovery reports a `pending` row as uncertain and keeps
its original worker fence. Only the exact permit an attempt was charged with records
its result, and a late result is still recorded after the lease moved. Committed
evidence must name exactly the attempts charged for its decision, locked for the commit,
each with its durable status and with its questions in the evidence's question catalog.
It must keep their input commitment and mode, keep
restrictions at least as strict as their binding restrictions, and carry exactly the
answers its answered attempts recorded. A rejected snapshot skips replay, because an
early failure has no answers or branches yet, but it must still carry its decision
identity and pass `Evidence::validate_rejection`: a reason other than `fits`, the
supported evidence version and contracts, restrictions within the policy floor, the
pinned Decider and model, a consistent retained-state reference, and fit counts of a
context that did not fit. An `insufficient` rejection only follows a complete
evaluation, so it must replay in full.

An applied decision saves the Run's context through the same lease-fenced save as an
adopted Summary Stage (`context.compacted`), which ends the step, and writes its
Decision Evidence in that transaction; the saved context must match the projection
digest its evidence names. A staged decision is recorded only by
`Store::adopt_compaction`, inside the transaction that saves the adopted summary, and
only when the adopted history keeps, in order, the pruned entries the summary did not
absorb. Shadow and rejected decisions never touch the Run. The lease is checked again
by the clock right before every commit, and retained state is refused if its deadline
passed while it was written, so a retry records its expired marker instead. Applying context or retaining state rechecks the current policy inside the commit
transaction and is forbidden once that policy no longer names the admitted pin;
evidence without either effect is still recorded. Retained state must match its digest,
and an expired marker must match its retention witness and the evidence's state digest,
and its deadline must have passed by the commit.
A decision has at most one applied, staged or shadow snapshot; each retry of a rejected
decision appends another. Retained state whose deadline passed before the commit keeps
only its expired marker, and `Store::expire_decision_states` later deletes due state and
writes the matching marker into its evidence. Decision Evidence is not exposed through
the public API.

Native execution builds the Decider's ports in `Environment::compaction_decider`
(`server/src/apps/execution/repositories/agent.rs`): `JevDecisionProvider` with the
version's byte budget as its capacity, the `CatalogAuthority` that requires the exact
version's Catalog Approval (`decision.invoke`) and grants the state retention the
version declares, and the Run's `RunDecisionJournal`.
