# Stints

A **Stint** is one Agent's unbroken stretch of work on one Task, from waking
until it goes to sleep ([ADR 0141](../adr/0141-a-stint-is-one-awake-stretch-and-everything-else-of-a-run-belongs-to-the-task.md),
[#1009](https://github.com/kent8192/aidash/issues/1009)). One Task spans many
Stints: an Agent that pushes a fix and waits an hour for CI sleeps in between,
and the CI result starts its next Stint.

## When a Stint begins and ends

| The Run…                                                                                                                      | Stint                                       |
| ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| is claimed, becomes ready, or starts thinking or calling a tool, with no Stint open                                           | opens                                       |
| receives a person's input while working                                                                                       | stays open                                  |
| is taken over by another worker after a crash (Lease Recovery)                                                                | stays open, and counts the takeover         |
| waits for a person's answer, an approval, a reconciliation, child or dependency Tasks, a command it started, or its own timer | ends, with that wait's name as its reason   |
| waits for a failure to be delivered                                                                                           | stays open: that is the worker's own retry  |
| is paused by a person, a Budget set to "stop" or a Safety Stop                                                                | ends, with `paused` as its reason           |
| is resumed                                                                                                                    | opens                                       |
| completes, fails or is cancelled                                                                                              | ends, with the terminal phase as its reason |

The database records Stints from the Run's phase and wait: a trigger on `task_executions`
opens and ends them for every writer, so no code path can skip one, and at most
one Stint per Run is open. A Stint also records the last worker lease
(`lease_owner`, the holder's claim token) and how many times another worker took
the lease over (`lease_recoveries`). A paused Run that is cancelled opens no
Stint: its cancellation is delivered without the Agent. Each Run-step
Inference Attempt records its Stint, making cost per awake stretch visible.

## Model Calls

A Model Call records a request for a compaction summary, a memory role, a
Workbench test, a media admission check or an MCP sample
([ADR 0144](../adr/0144-native-is-the-only-projection-version-and-model-requests-outside-run-steps-are-model-calls.md)).
The request uses Native model input. The record contains the model version, a
SHA-256 digest of its canonical input, the outcome, reported usage and cost,
and the start and end times. Prompt and response contents are excluded from the record.

An awake execution's Stint owns its Model Calls. Other work belongs to its
Tenant and Node. Workbench tests use the Tenant's default Node. Memory retries
of the same request have separate Usage Records under the original Model
Call. MCP sampling keeps its Run Budget attribution. A media admission check
measures a proposed request locally and has no provider usage.

`GET /api/executions/{id}` returns each Stint with its `model_calls` array.
`GET /api/tenants/{tenant}/administration` returns `model_call_usage` by
purpose for callers who can manage the Tenant's Budgets.

## What stays with the Task

The Context Journal, the delegation chain and requester, the Completion Gate's
progress, people's pause and cancel, budgets and grants that last until the next
user input live with the Task's execution record and carry across Stints: a
woken Agent continues where it slept.

## Limits

Runs have no step limit; an Agent definition's `max_steps` is accepted for
stored definitions and ignored. Other limits on one Agent were widened with
Stints:

| Limit                                                    | Value                                                                                         |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Command Time Limit                                       | 10 minutes by default; a Deployment's capability profile may raise `maximum_seconds` to a day |
| Consecutive Lease Recoveries before a Crash Loop Failure | 5 by default; a Tenant may set 1 to 20 in its execution settings (`tenant.update`)            |
| Waits for an approval (`approval_seconds`)               | 7 days by default; a Deployment may set up to 30 days                                         |
| An Agent definition's Tool Parallelism                   | up to 64                                                                                      |

## Names

The Task's execution record is the table `task_executions` (formerly `runs`).
API paths say `executions` where they said `runs`: `GET /api/executions/{id}`,
`POST /api/executions/{id}/control`, `GET /api/tasks/{id}/executions`,
`GET /api/threads/{id}/executions`, `GET /api/suspended-executions` and the
other `/api/executions/{id}/...` routes, with operation ids such as
`execution-get` and `task-executions`. Events say `execution.` where they said
`run.` (`execution.started`, `execution.control`, `execution.budget_exhausted`,
...), with the same meaning and data; stored events were renamed with them.

Internal names stay: the `run_id` columns and fields, the Rust types (`Run`,
`RunRecord`, ...), the policy actions `run.read`, `run.message`, `run.control`,
`run.decline_call` and `run.finish`, the resource kind `run`, and the keys
`runs` and `run_id` inside API bodies such as `/api/state`.
