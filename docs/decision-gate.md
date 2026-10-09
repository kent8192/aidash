# Portable decision gate foundation

The portable `DecisionGate` provides explicit, version-pinned Compaction decisions
for #108. It is not composed into native agent execution yet. Native authority,
owner accounting, atomic persistence and administration adapters remain required.

Decider declarations require an exact Jev model version and an HTTPS endpoint.
HTTP is accepted only when the URL host is a literal loopback IPv4 or IPv6
address. DNS names, including `localhost`, cannot authorize plaintext disclosure.
The Jev adapter validates the declaration before resolving credentials or
preparing authenticated requests.

`DeciderConfig::digest()` commits to the canonical JSON object containing `model`
and `parameters_digest`. The latter hashes the complete serialized configuration
with `model` replaced by JSON `null`. Historical `Evidence` retains that hash as
`configuration_parameters_digest`, allowing replay to reconstruct and check the
configuration pin using its recorded model. Endpoint and credential references
are not copied into evidence. No current definition, policy or provider call is
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
default a missing commitment. This foundation provides no legacy compactor
projection or native cutover.

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

Decision identities commit to the Node, Run, step, input revision/digest, exact
Decider pin, Binding restrictions, classification state, sources and target window.
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

Evidence version 2 requires an explicit `state_retention` witness. Retained and
expired markers must match its immutable state ID and deadline and the existing
state digest. Cleanup can remove bytes and replace a retained marker with its
matching expired marker, but cannot erase the witness or mark retained state as
`Disabled`. Earlier evidence versions are rejected rather than defaulting a witness.

Rust adapters must update `Evaluation::decider` to accept `&BoundDecider`, include
`DispatchRecord::binding_restrictions`, implement `DecisionJournal::recover`, and
return `Reservation` from `reserve`. Handle `AttemptStatus::NotDispatched`, the new
failure reasons and the required `Evidence::state_retention` field. Native Gate
composition and durable database/worker acceptance remain pending.
