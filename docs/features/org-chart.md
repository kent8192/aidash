# Org chart

The org chart shows what each request's Agents built, under whose authority
and with what spend, and stops all of it with one action (#1012, ADR 0143).
Agents may create Nodes, Duties, Subnodes and generated agents without a count
limit; the org chart is how people see and stop what grew.

## Flows

A **flow** is one Root Task (a request) and everything created under it:

- its Task tree;
- the Tasks its Runs created or delegated;
- the Duties its Runs set up, and the Tasks those Duties started;
- the Subnodes and the Nodes its Runs created.

A Task records nothing extra for this: the links already exist (Task parents,
the Run that created a Task, the Task whose Run set up a Duty, the Task whose
Run created a Subnode or a Node).

## Reading the chart

Settings → Tenant → **Org chart**, or
`GET /api/tenants/{tenant}/org-chart`, lists each Node the person may read
(`node.read`) with:

- its label and who created it: a person, the Tenant's creation, or an Agent
  acting for a root person, with the creating Task;
- its Duties: title, Requester, Agent and state (`active`, `paused`,
  `stopped`);
- its active Subnodes: the creating Agent, the root person of its chain and
  its members;
- the Root Task of the flow that made each Agent-created item;
- this month's spend (UTC), from the Budget ledger.

What the person sees follows the Budget rules:

| Viewer                                                            | Duties and Subnodes                                                     | Spend                                       |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------- |
| A Node's administrator (`node.administer`, Tenant Administrators) | all of that Node                                                        | that Node's and its Duties'                 |
| `duty.administer`                                                 | every Duty                                                              | none beyond their own                       |
| `budget.manage`                                                   | as any other viewer                                                     | every Node, Duty and person                 |
| anyone else                                                       | Duties they request, Subnodes of their own chains; the rest are counted | their own Duties' and their own as a person |

A browser session is required, as for Budgets.

## Stopping a flow

**Stop this flow** on the org chart, or on a Root Task in the Task view, calls
`POST /api/tenants/{tenant}/flows/{root_task}/stop`. It:

- cancels every open Run of the flow (a cancelled Run also ends the Subnodes it
  created);
- ends the flow's Subnodes that outlived their creator;
- pauses the flow's Duties; only their Requester resumes them;
- leaves the Nodes the flow created in place and lists them, since they belong
  to the Tenant. A Tenant Administrator deletes them if they are unwanted.

Each item is stopped on its own; the answer says for each Run, Subnode and Duty
whether it was `stopped`, `already_stopped` or `failed` with the error, so one
failure never keeps the rest running. Whoever may control a Run of the Root
Task (`run.control`) or administers the Root Task's Node stops it, from a
browser session; no Agent does.

## Nodes Agents create

An Agent creates a Node with `node_create` only when the person at the root
of its chain holds `node.create`, and deletes one with `node_delete` only when
a chain of the same person created it or that person holds `node.delete`. See
[authorization](../authorization.md).
