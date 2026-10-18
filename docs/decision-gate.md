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

Decider declarations require an exact OpenRouter Jev release (such as
`typesafe/jev-1.14`; aliases are refused) and name `provider_credential:
"openrouter"`. A version also declares its per-Run call limit
(`max_calls_per_run`, which a Binding can only lower), whether it keeps the
classification state (`state_retention`, allowed by the version's Catalog
Approval) and its `state_budget`: the bytes of state and request Jev's default
mode takes, the long-context limits, and the tool-input abridging stages. The
builder `aidash.compaction/2` abridges disclosed tool inputs stage by stage until
the state fits the default mode, sends only the most abridged state in the
long-context mode, and refuses a larger one with `decider_unavailable`. On the
`native` Projection Version it protects whole Model Turns. They configure no endpoint or key: the Jev adapter calls
OpenRouter's System One API with `provider.zdr = true`, resolving the Tenant's
current OpenRouter Provider Credential for each dispatch rather than when a
request is prepared, so a revoked or unbound credential stops later attempts
(ADR 0041). A response must name the pinned release or a dated snapshot of it.
The adapter validates the declaration before preparing requests.

`DeciderConfig::digest()` commits to the canonical JSON object containing `model`,
`mode` and `parameters_digest`. The latter hashes the complete serialized
configuration with `model` and `mode` replaced by JSON `null`, so evidence that
sent no request still proves the mode its Decider ran in. Historical `Evidence` retains that hash as
`configuration_parameters_digest`, allowing replay to reconstruct and check the
configuration pin using its recorded model. Credentials are never copied into
evidence. No current definition, policy or provider call is
needed to verify the commitment.

Replay checks dropped counts against the recorded branches and requires the
truncated count to be no greater than the number of `TruncateResult` branches.
Those branches can leave short results unchanged, so their count is an upper
bound rather than an exact truncation count. The complete candidate must fit
before Enforce can apply it; Shadow records the proposal without applying it.

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

Decision identities commit to the Node, Run, step, the Run's resume cycle, input
revision/digest, exact Decider pin, Binding restrictions, classification state,
sources and target window. The resume cycle grows each time a person resumes a
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
