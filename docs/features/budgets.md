# Budgets

Budgets are the money brake on Agents (#1012, ADR 0143). Nothing else limits
how many Agents, Subnodes, Nodes or Duties an Agent's chain sets up; a Budget
limits what they spend on model calls. Budgets are optional.

## Scopes

A Budget covers one of:

- **the Tenant**: every model call of the Tenant;
- **a Node**: calls of Runs executing on that Node;
- **a Duty**: calls of the Duty's Tasks and of every Task their Runs create;
- **a person**: calls of every chain whose root person they are, so
  everything their Agents set up for them counts.

A scope has at most one Budget. A model call counts against every Budget that
covers it.

What an Agent creates is carved out of its creator's Budget (decision 33): a
Duty that a Run of another Duty set up sits below that Duty, and a Node that a
Run on another Node created sits below that Node, however deep. A Duty or Node
Budget covers, and counts the spend of, everything below it. The lineage is
written once when the Duty or Node is created and outlives both, like the
ledger, so deleting a Duty in the middle frees nothing below it. A Duty's
deletion can ask which Budgets cover it, ancestors included.

## Limits and windows

- **Soft Limit**: when this window's spend reaches it, Aidash mails the
  Budget's recipients once for the window, through the Deployment's SMTP
  settings. Recipients are chosen or typed by whoever sets the Budget.
- **Hard Limit**: when spend reaches it, the next model call of a covered Run
  is refused before it is sent, and no covered Duty starts a Task.
  - **Stop** (the default) pauses the Run until a person resumes it.
  - **Sleep** lets the Run wait, rechecked every hour, until the window resets
    or the limit is raised; then the same step runs.
- **Window**: each calendar month in UTC, or the total since the Budget was set.

Every model call passes the gate: a Run's own calls, MCP sampling, and the
calls of memory roles (reflection, reranking, extraction, retention) and
memory embeddings. Memory work outside any Run, such as background
maintenance, is charged to its Tenant and its Thread's Node; a background job
the gate refuses waits without using an attempt. Workbench tests charge the
Tenant and its default Node. A media admission check runs locally without
provider spend.

Spend is the cost providers report, in credits (one credit is one US dollar on
OpenRouter). A call without a reported cost, such as an embedding, is counted
as unpriced and shown beside the Budget, but cannot reach a limit. A call
already sent when the limit is reached completes, so spend can pass a Hard
Limit by that call. A call whose Run lost its lease while it was in flight
still posts its cost when it finishes.

## Who sets Budgets

Only from a browser session, so no Agent sets or raises one:

- `budget.manage` (Tenant Administrators) sets any Budget;
- a person sets their own;
- `node.administer` on a Node sets that Node's;
- a Duty's Requester sets that Duty's.

The API is `GET|POST /api/tenants/{tenant}/budgets` and
`PUT|DELETE /api/tenants/{tenant}/budgets/{budget}`; see
[authorization](../authorization.md).

## Spend ledger

Spend is kept in a ledger per day, Node, root person and Duty, apart from the
usage records of Runs.

A Soft Limit mail the SMTP server refuses is not recorded as sent; the next
pass sends it again. Deleting a Duty or a Thread, or purging a Node, removes
Runs but gives no money back to the Budgets that covered them.
