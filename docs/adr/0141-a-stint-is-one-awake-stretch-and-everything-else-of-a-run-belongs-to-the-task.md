# A Stint is one awake stretch, and everything else of a Run belongs to the Task

Decided on 2026-11-17 and 2026-11-18 in the agent activation grilling (umbrella Issue #1007). "Run" was defined as "one attempt by one agent version at one Task", yet the code keeps one `runs` row per Task and Node (`runs_home_node_task_id_key`), and that row spans every wait from claim to completion. Nobody could say when a Run began or ended. Agents that sleep between events, for example while CI runs for an hour or an approval waits three days, need a clear unit of being awake.

## Decisions

- **A Stint is one Agent's unbroken stretch of work on one Task, from waking until it goes to sleep.** It ends when the Task enters a wait the Agent cannot move forward itself: a person's answer or approval, a reconciliation, child or dependency Tasks, a command it started, an outside result, or a timer it booked. A wait for a failure to be delivered stays inside the Stint. Input arriving while it is awake and a takeover after a crash (Lease Recovery) stay inside it. One Task spans many Stints. Whether a wait of a few seconds really releases the worker is an implementation optimisation.
- **The word Run is retired.** Today's `runs` row becomes the Task's execution record at a Node (`task_executions`), keyed as before by Task and home Node, and keeps everything that lives across Stints: the delegation chain and requester, the Completion Gate's progress, people's pause and cancel, budgets, the Context Journal, grants that last until the next user input, and every table that references it today.
- **A new `stints` row records each awake stretch**: the execution it belongs to, the Agent version and Node that worked, the worker lease, the Lease Recovery count, when it started and ended, and why it ended. Inference attempts and usage record their Stint.
- **A woken Agent continues where it slept.** A new Stint resumes from the Task's Context Journal and saved state; nothing restarts blank.
- **`max_steps` is removed.** Single-agent runtime caps that remain (command Time Limit, outage back-off, crash-loop limit, waits for people and MCP) are widened and apply per Stint where they measure a stretch of work.

## Considered Options

- **Rename `runs` to `stints` and add a Task-level record.** Rejected: about 45 tables and every authority, journal and gate record already key on the `runs` row, which is Task-level by construction; moving them gains nothing.
- **Keep the word Run for the Task's execution record.** Rejected: the user found the word's span unclear, and keeping it beside Stint would leave two words for overlapping things.
- **End a Stint on every wait.** Rejected: a wait for failure delivery is the worker's own retry, not sleep.

## Consequences

- API paths and event names that say "run" are renamed; Aidash is not deployed, so no compatibility layer is kept.
- Dashboards show Stints inside a Task's history, which makes cost per awake stretch visible.

## Amendment (2026-11-20, implementation of #1009)

Calls made while renaming, recorded here so the decisions above are read with them:

- **The table, the API paths and the event names follow; internal names stay.** `runs` is `task_executions` (same rows, keys, policies and triggers; its keys, checks and indexes take `task_executions` names). Every `runs` path segment under `/api/` is `executions` (`/api/executions/{id}/...`, `/api/tasks/{id}/executions`, `/api/threads/{id}/executions`, `/api/threads/{id}/agents/{agent}/executions`, `/api/suspended-executions`), and route names follow (`run-get` is `execution-get`, `task-runs` is `task-executions`, and so on). Every `run.*` event is `execution.*` with the same meaning and data, and stored events were renamed with them.
- **Kept, because they are internal or stored**: the `run_id` columns and fields of other tables and API bodies, the `run_*` table names, the Rust types (`Run`, `RunRecord`, ...), trigger and function names, the keys `runs` and `next_cursors.runs` in `/api/state`, the resource kind `run` and the policy actions `run.read`, `run.message`, `run.control`, `run.decline_call` and `run.finish` (stored in policies; renaming them needs a policy migration of its own), the Runner protocol (`/v2/runs/...`), stored resource and evidence keys (`{node}/runs/{id}`, `aidash://home/runs/{id}`), the federation report `remote.run.recovered` and the event `task.remote_run_recovered`, and the people-facing word "Run" in the dashboard.
- **`run.control` was both an action and an event.** Only the event became `execution.control`; the action keeps its name.
