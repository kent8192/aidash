# A refusal after the gate pauses only when a person on the Run's Node can lift it

Decided on 2026-11-06 for #754. #529 and #530 settled refusals at the pre-dispatch gate, but refusals from a tool's effect still paused the Run with no reason or failed it outright. The model never heard about them, and resuming repeated the same call. In one multi-agent build a Run stayed paused for hours on a refusal no person could lift.

A tool call refused after the gate now ends in one of these ways:

- **Authority Pause.** A policy denial or a Catalog Approval refusal on the Run's own Node, including for the Agent it delegates to there. The pause names the tool, the check and, when it is not the Run's own, whose authority refused. A Tenant Deletion also pauses, and so does a recheck after the effect that finds the Run's own authority gone, because the effect has already happened and telling the model "refused" would be false.
- **Rejected Call.** Everything else that is refused, including a `Forbidden` nothing has classified. The model gets a sentence saying so. The sentence also says the call may have partly happened when the tool is `Unsafe` or is known to work in several steps. A denial from another Node stays a Rejected Call, as ADR 0109 decided.
- **Run failure.** A `Conflict` the call did not cause.

An unclassified refusal goes to the model rather than pausing. A pause that no change of authority can lift is the failure this decision exists to remove, and no authority is lost: authority is checked again before every model request, so a Run whose authority is gone stops there. The cost is that a refusal a person could have lifted, if still unclassified, reaches the model without waiting for them.

`NotFound` becomes a call error everywhere, but `Conflict` only where the call caused it. `Conflict` also carries internal faults such as `admitted Binding definition changed`, and returning those to the model would let the Run go on with a stale definition.

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
- A denial the code deliberately ignored can leave a note behind. If a later plain `Forbidden` ends the same call, the call pauses once under that name, and on resume it gets a fresh note and the right answer.
- A plain `NotFound` never reads the note, so a Run does not pause on a genuine "file unavailable" after an ignored denial.
- A leftover `Forbidden` does not know where it came from, so its log line names only the Run, the tool and the call.
- A person cannot yet decline a paused call and let the Run go on. That is #763.
