# 0161. A Goal judged by a separate model replaces the Completion Gate

- Status: Accepted
- Date: 2026-11-26
- Issues: #698, #1019, #1021, #1023

## Context

The Completion Gate runs deterministic Checks against a registered Working Area.
That works for mechanical checks, but Aidash Tasks also produce proposals,
research and replies whose completion depends on what the requester asks for.
The Working Area and the Artifact copy of the final answer are being removed.

## Decision

A Task may have one Goal of up to 4,000 characters. The requester, the Duty
procedure or the Agent that creates the child Task supplies it. Editing a Duty
procedure only changes the Goals of future Tasks.

When the working model stops with an answer and all waits have settled, Aidash
makes a separate, tool-free Model Call with the Goal and the compacted
transcript. The executing Tenant selects and pays for the judge. An unset or
removed judge model falls back to the working model in a fresh call. The call
and its usage are recorded under `goal_judge` and count against the Task's
applicable Budgets.

A `met` verdict completes the Task. A `not_yet` verdict supplies guidance for
another turn. Three consecutive such verdicts without intervening tool use
pause for the requester. An `impossible` verdict ends the Task as failed and
includes its reason in the Completion Message. The requester may revise the
Goal and resume with the same context. Unparseable responses count as failed
calls. A retry that also fails pauses for the requester. Budget exhaustion
uses the existing wait behavior.

A Task without a Goal completes on the working model's final answer. In either
case the completion is one Thread message per execution attempt. It carries
the final answer, any terminal verdict and the references in the Deliverable.
Events identify that message by ID. Federation carries the Goal to the executor
and the Completion Message back to the requesting side.

## Consequences

Task detail exposes the Goal edits and verdict history with model, time and
reported cost. The judge has only transcript evidence. A Goal can tell the
working Agent what evidence to obtain and present in its transcript.
