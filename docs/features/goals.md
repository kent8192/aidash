# Task Goals and Completion Messages

A Goal is an optional completion condition of up to 4,000 characters. Add it
when requesting work in a Thread, when defining a Duty that starts Tasks, or
when creating a child Task.

Aidash asks a separate model to judge the Goal whenever the working Agent
stops with a final answer and its outstanding waits have settled. The judge
sees the Goal and the transcript after compaction. A Task without a Goal
completes with the Agent's final answer.

| Verdict    | Task behavior                                                                                                                               |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Met        | The Task completes and posts a Completion Message.                                                                                          |
| Not yet    | The Agent continues with the judge's guidance. Three consecutive verdicts without tool use pause for the requester.                         |
| Impossible | The Task fails and the Completion Message includes the judge's reason. The requester can rewrite the Goal and resume with the same context. |

The Task detail shows the current Goal, its edit history and each verdict.
Verdicts include the judge model, time and reported cost. The Tenant
Administrator can choose a registered model in Execution settings. Aidash uses
the working Agent's model when the setting is empty or names a removed model.
A failed call is retried once before Aidash pauses and asks the requester.
All judge calls count against the Budgets covering the Task. For a federated
Task, its executing Tenant provides the judge and pays for the calls.

The Completion Message holds the final answer, the verdict if present, and
the Task's Deliverable. A Deliverable can include published Drive versions,
Outside Versions recorded by the Agent, and up to 256 selected Scratch Space
files totaling 4 GiB. Ordinary Thread messages retain their existing limits.

See [ADR 0161](../adr/0161-a-goal-judged-by-a-separate-model-replaces-the-completion-gate.md).
