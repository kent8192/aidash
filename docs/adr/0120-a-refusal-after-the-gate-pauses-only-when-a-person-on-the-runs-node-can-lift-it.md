# A refusal after the gate pauses only when a person on the Run's Node can lift it

Decided on 2026-11-06 for #754. #529 and #530 settled refusals at the pre-dispatch gate, but refusals from a tool's effect still paused the Run with no reason or failed it outright. The model never heard about them, and resuming repeated the same call. In one multi-agent build a Run stayed paused for hours on a refusal no person could lift.

A tool call refused after the gate now ends in one of these ways:

- **Authority Pause.** A policy denial or a Catalog Approval refusal on the Run's own Node, including for the Agent it delegates to there. The pause names the tool, the check and, when it is not the Run's own, whose authority refused. A Tenant Deletion also pauses, and so does a recheck after the effect that finds the Run's own authority gone, because the effect has already happened and telling the model "refused" would be false.
- **Rejected Call.** Everything else that is refused, including a `Forbidden` nothing has classified. The model gets a sentence saying so. The sentence also says the call may have partly happened when the tool is `Unsafe` or is known to work in several steps. A denial from another Node stays a Rejected Call, as ADR 0109 decided.
- **Run failure.** A `Conflict` the call did not cause.

An unclassified refusal goes to the model rather than pausing. A pause that no change of authority can lift is the failure this decision exists to remove, and no authority is lost: authority is checked again before every model request, so a Run whose authority is gone stops there. The cost is that a refusal a person could have lifted, if still unclassified, reaches the model without waiting for them.

`NotFound` becomes a call error everywhere, but `Conflict` only where the call caused it. `Conflict` also carries internal faults such as `admitted Binding definition changed`, and returning those to the model would let the Run go on with a stale definition.

An exact definition absent from the caller's Tenant Registry is a `NotFound`, including when `task_delegate` names an unknown Agent at the gate. A registered version without Catalog Approval still pauses. A scope inheriting a lease does not check Registry existence outside its inherited approval frontier, so its refusal remains an Authority Pause.

A paused refusal of a replay-safe tool ends with status `REFUSED`, and resuming runs it again. An `Unsafe` tool keeps the older path, in which resuming asks a person to reconcile the call, because running it again could repeat an effect. A person may sometimes be asked about a call that never ran. That is better than a second charge or a second Live preview.

Checks record why they refused in a note scoped to the tool call, and the harness reads it when the call ends. `Forbidden` keeps its meaning for about 270 match arms that treat it as "skip" or "not visible".

## Considered Options

- **Pause on every refusal and name the tool.** Every pause would have a name, but resuming would still repeat a refusal nobody can lift. Rejected.
- **Classify only by whether a denied decision row exists.** The Catalog check returns before writing one, and `task_visible` writes denied rows for tasks that must read as a Rejected Call. Rejected.
- **A new error variant from `Access::require`.** It is explicit, but any match arm missed among the 270 would silently change what lists and visibility checks return. Rejected.
- **Return every `Conflict` to the model and split out internal faults.** A missed fault would let a Run continue on a stale definition. Keeping them fatal fails safe. Rejected.
- **Record every refusal in `authorization_decisions`.** The table had already reached 15 GB (#758). Refusals without a policy decision write a log line instead. Rejected.

## Consequences

- The note does not survive a spawned task. A refusal raised there becomes a Rejected Call.
- A denial the code deliberately ignored can leave a note behind. If a later plain `Forbidden` ends the same call, the call pauses under the ignored denial's name. Resuming runs the call again, which ignores the same denial and pauses again, so a tool that swallows a denial must not end in a plain `Forbidden`.
- A plain `NotFound` never reads the note, so a Run does not pause on a genuine "file unavailable" after an ignored denial.
- A leftover `Forbidden` does not know where it came from, so its log line names only the Run, the tool and the call.
- A person cannot yet decline a paused call and let the Run go on. That is #763.

## Implementation

- The note is a task-local cell in `aidash-application` (`authorization/refusal_note.rs`). The harness scopes one around every tool effect (`invoke`, and each `dispatch` of a Tool Batch) and reads it when the effect ends, and scopes another around a whole worker step, from the step gate through recovery, so `recover` can name a refusal raised outside a tool call (inference, compaction, worker entry, `human_read`) after the stage the Run was in. Outside a scope the note is a no-op, so HTTP paths are unchanged.
- `Access::require` sets a `Policy` note on denial and clears a `Policy` note on allow, so the last check wins. `decide` never does, because `task_visible` uses it. `subnodes::create` evaluates policy itself and sets the note there. Catalog admission sets a `Catalog` note at each of its refusals. A Tenant Deletion sets its note where `TenantDeleted` becomes `Forbidden`, which covers every source of that error at once.
- The Agent being delegated to is the subject an execution admission appends to the chain. `Access` remembers it, and a denial names it only when the policy denies that subject.
- `reference.read` for the delegated Agent's attachments uses `get_for_admission`, which keeps `Forbidden`. A real `task_delegate` reaches it through `references::pin`; the Reference API, the management endpoint that configures attachments and the session source check still use `get`, which answers 404.
- A peer's 404 is `NotFound` in `federation::authority::classify`, but the server's `authority_request` folds it back to `Forbidden`, as it has always read, because most callers branch on that. Only the two callers whose result reaches the model keep it: `HomeRepository::request` and the Remote Run's `command()`. A Remote Grant Run's authority refresh therefore still pauses on a 404.
- `invocations.status` gains `REFUSED` in two migrations: `0120` for the database CHECK and `0121` for the ORM state, because a migration is either database-only or state-only.
- `Access::require` sets no note when the refusal comes from the Subnode read-only rule. No change of policy lifts that rule, so the call reaches the model as a Rejected Call (#755).
- A step that is not a tool call, or a tool call whose refusal nothing named, is reported as `<stage>: <check>`. The stage is the tool being called when its name is a plain identifier, else the Run's phase (`model call`, `compaction`, `worker start`, `resume`); a refusal inside compaction, whether the pruning decision or the summary, is named `compaction`. A refusal with no note reads `<stage>: an authority check refused this step`.
- A refusal or failed request after a cross-Node admission (`task_delegate` to another Node) is a Rejected Call that says the delegation may have partly happened, because pausing would resume into a second admission. That covers a denial or a 404 from the activation, an unreachable Node, and a refusal by this Node while it records the admission. An ask from the target Node still pauses, as ADR 0109 decided. A `Conflict` there still fails the Run, and a pending transaction or a database error keeps its own retry path.
