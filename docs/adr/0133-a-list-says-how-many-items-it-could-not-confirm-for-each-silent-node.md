# A list says how many items it could not confirm for each silent Node

Decided on 2026-11-07 for #770. #751 left an item out of a list, search or tool result when a Node holding one of its Read dependencies did not answer, and a person had no sign of it: a Workspace with 100 Tasks could show 97 and look complete.

Every HTTP list and search response that applies this rule carries `unconfirmed`, always present and `[]` when empty. Agent tool results carry it with a note only when it is non-empty. Each entry is `{node, kind, workspace_id, count}`: the Node's address, one of `task`, `run`, `artifact`, `event`, `message`, `human_request`, `semantic_entry`, `memory_unit`, the Workspace, and how many rows this response scanned and could not confirm. `/api/state` mixes kinds and the Web narrows it by Workspace in the browser, so both are part of the entry.

- **Three results for a row.** Visible, not visible, cannot confirm. Only the last is counted, and a real refusal never is. A row counts only after the reader's own permission to read that kind in that Workspace and `federation.discover` on that Node have passed, so a reader without them learns nothing.
- **Every other check first.** Where a check that can meet a silent Node sits in a chain with others, the rest of the chain is judged before the Node is kept, and a row that any of them refuses is a refusal. The rest is judged only when a Node was silent, so rows the first check hides plainly cost no extra Authorization Decisions.
- **A count, not a total.** It is the rows the response scanned. It is not "you should see these", because some are refused once the Node answers. A row counts once, under the first silent Node its check reached.
- **Shapes.** `/api/events`, `semantic/entries` and `memory/units/query` were bare arrays and become `{events|entries|units, unconfirmed}`. The event cursor stays in its header. `WorkspaceSnapshot` gains the field with a serde default because peers read it. The federated graph is left to #799, because a peer builds that answer and a third Node's address would cross Nodes to a person.
- **The event stream says nothing.** A skipped event is not sent again while connected, and a reconnect reads forward only. The Web uses frames only as a signal to re-read, so it learns from the next page, and `GET /api/events?after=` checks them again. Holding the cursor until a Node answers would be the whole-list failure #751 rejected.
- **A silent Node is remembered for lists only.** For 30 seconds, per Tenant and Node and per process, a list, search or event read does not ask a Node that did not answer, so a poll stops spending up to 10 seconds with a database connection held. An answer, including a refusal, clears it. A single read, a Read Recheck and a Run's inference stream always ask, because a person or a Run must never act on a guess. The memory can only produce "cannot confirm", so it can never show what a reader may not see.
- **A child Task that cannot be confirmed is not "no pending child".** It is `node_unreachable` before any effect, so it takes a Read Recheck's path: up to 5 retries, then a pause.

## Considered Options

- **Fail the whole list with `503`.** One Node's trouble would stop every list. Rejected in #751 and again here.
- **A boolean or one total.** A total needs every row checked on every request, and a boolean cannot say which Node to wait for. Rejected.
- **Hold the event cursor until the Node answers.** The same whole-list failure for the stream. Rejected.
- **Let single reads use the 30 second memory.** A person opening one thing would be told a Node is silent that may have recovered. Rejected.

## Consequences

- After a Node returns, items can stay out for up to 30 seconds.
- A row that depends on two silent Nodes counts under the one its check reached first, and can appear under the other once the first returns.
- A row whose 20 second limit ran out on a reference to this Node has no Node to name, so it is only logged. A row with no route stays a refusal.
