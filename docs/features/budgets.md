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

Spend is the cost providers report, in credits (one credit is one US dollar on
OpenRouter). A call without a reported cost is counted as unpriced and shown
beside the Budget, but cannot reach a limit. A call already sent when the limit
is reached completes, so spend can pass a Hard Limit by that call.

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
usage records of Runs. Deleting a Duty or a Thread, or purging a Node, removes
Runs but gives no money back to the Budgets that covered them.
