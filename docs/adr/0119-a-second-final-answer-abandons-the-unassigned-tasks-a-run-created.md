# A second final answer abandons the Unassigned Tasks a Run created

Decided on 2026-11-07 for #772. A Task can complete only once its children are completed or abandoned, and neither a model nor a person could close an open child that nobody was running. A Run in that state sent every final answer back to the model until `max_steps` failed it. #733 slows the loop to one call an hour, but the Run still never finishes.

When only Unassigned Tasks are left under a Run's Task, its final answer is held and not published, and the model is told which Tasks they are. It can hand them over. If it answers finally again, the ones created by Runs of that Task are abandoned in the transaction that completes the parent, and a fixed sentence naming them is added to the published answer and the result artifact. Unassigned Tasks that anyone else created, or that have open children of their own, go to a person.

We chose this over asking a person because a person asked would stop the Run with no deadline, and a Schedule's Run may have nobody watching it. The cost is that a model which ignores the notice drops work. That loss is bounded and visible: the model was told, the abandonment is recorded with a reason, and the sentence reaches anyone reading the result, including a coordinating Agent.

The model is told once per Task, not once per input as a citation correction is, so a Task created after a notice is never abandoned without its own warning.

## Considered Options

- **Abandon at the first final answer.** It costs no extra model call, but the model never learns that its answer leaves work undone. Rejected.
- **Ask a person after the notice.** No work is dropped silently, but the Run stops until someone answers, which is what the Approval Modes exist to avoid. A person is asked only about Tasks the Run did not create. Rejected.
- **A tool for the model to give up a Task.** It would let the model give a reason, but delegating or answering again already covers every choice, and a tool adds a contract and an authority surface to maintain. Rejected.
- **Let an Unassigned Task not block completion.** The parent would complete with an open child left behind, and the rule that a completed Task has resolved children would no longer hold. Rejected.

## Consequences

- A model that ignores the notice drops the work, and the result says so.
- Abandoning checks `task.abandon` like the other steps of completion, so a Tenant that denies it to an Agent gets an Authority Pause instead.
- When a person must answer about an Unassigned Task at the step limit, the hold does not consume the last step; the Run can think again after the person answers.
- A child on another Node that waits on an abandoned sibling fails, and the parent then reaches the existing failed-child path. Here the parent cannot complete, so the abandonment commits on its own, and that commit usually wakes the parent for one extra model call. The Home returns the Task ids it actually abandoned, and the Run keeps only those across that wait and names them in its final answer; a Task handed over after the summary is left out. An older Home that returns no Task ids cannot provide this accounting, so the receiver rejects that response instead of claiming an abandonment. A local child cannot be in this state, because local delegation refuses a Task whose dependencies are unfinished.
- If a local Home commits an early abandonment but the Run stops or loses its lease before saving the returned IDs, retry no longer finds those children as Unassigned. The final answer can omit them; the `task.abandoned` event remains. Remote Home replay retains the returned IDs.
- If a person hands an Unassigned Task to an Agent at the step limit, the subsequent wait for that child consumes the last step. The Run can then fail on `max_steps` before reading the child result.
- The answer is published just before completion commits. A person handing over a Task in that instant can leave a published sentence naming a Task that was not abandoned.
- While a child is still running, the internal wait no longer calls the model at its ceiling. A Run whose child never changes waits, at no cost, until a person cancels it.
- A person can abandon an Unassigned Task, so a failed parent with one is no longer stuck.

## Implementation

- `aidash_domain::unassigned` holds the rules: `classify` turns a parent's pending children into Unassigned Tasks, and `decide_children` chooses what a final answer does. The local store, the guarded read and the Node-to-Node read all build `ChildTaskSummary.unassigned` through `classify`. The SQL that says whether a child has started or may be abandoned lives in one place, `server/src/apps/workspaces/models/child_flags.rs`.
- A Task has started when a Run of it is under way, a generated specialist is still coming for it, or another Node confirmed the Run there started. That last needs `authorization_remote_execution.activated_at` (migration `identity/0072`), because the binding row is written before activation and survives a refused one. The binding counts only while it carries the Task's current revision, so a reopened Task becomes Unassigned again.
- The notice is the Context event `UnassignedTasksTold`. It stays in requests across turns with calls, like a citation correction, and the Tasks it names are never named again.
- `ExecutionHome::complete` takes the ids to abandon, and the store abandons them after the key replay check and before the unresolved-children check, re-checking each inside its row lock. `ExecutionHome::abandon_unassigned` abandons without completing, for the other-Node case above.
- The saved completion records its list in `ToolCallState.abandoning`, behind the `unassigned_abandonment` persisted capability, so a cancel settles exactly that outcome and the settlement check accepts the list only as saved.
- A Run on another Node does not check `task.abandon` itself, because it can name only its own Task. Its Home checks it for the Run's chain when the command arrives.
