# Org chart

The org chart shows what each request's Agents built, under whose authority
and with what spend, and stops all of it with one action (#1012, ADR 0143).
Agents may create Nodes, Duties, Subnodes and generated agents without a count
limit; the org chart is how people see and stop what grew.

## Flows

A **flow** is one Root Task (a request) and everything created under it:

- its Task tree;
- the Tasks its Runs created or delegated, including those handed to another
  Deployment through a Remote Grant;
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

`node.read` on a Node shows the Node itself, not the work on it:

- a **Duty** shows to whoever its own page shows to: its Requester, the
  successor named to take it over, whoever may read its Thread
  (`thread.read`) and holders of `duty.administer`;
- an **active Subnode** shows to the root person of the chain that created it
  and to the Node's administrators (`node.administer`, which Tenant
  Administrators hold);
- the rest are only counted (`hidden_duties`, `hidden_subnodes`), so a member
  sees that other people's work exists without seeing what it is.

Spend follows the Budget rules: it shows only to whoever may set that scope's
Budget, and only on the Duties the person sees.

| Viewer                                                            | Spend                                       |
| ----------------------------------------------------------------- | ------------------------------------------- |
| `budget.manage`                                                   | every Node, Duty and person                 |
| A Node's administrator (`node.administer`, Tenant Administrators) | that Node's and its Duties'                 |
| anyone else                                                       | their own Duties' and their own as a person |

A browser session is required, as for Budgets.

## Stopping a flow

**Stop this flow** on the org chart, or on a Root Task in the Task view, calls
`POST /api/tenants/{tenant}/flows/{root_task}/stop`. It:

- cancels every open Run of the flow (a cancelled Run also ends the Subnodes it
  created);
- cancels the work the flow handed to other Deployments: each Remote Grant is
  revoked, so it renews no more, the receiving Node is asked to cancel its Run,
  and the Task here is cancelled once the receiver accepts. This runs as the
  system, as a Node deletion's cancellation does, so whoever stops the flow
  need not be the grant's root person;
- ends the flow's Subnodes that outlived their creator;
- pauses the flow's Duties; only their Requester resumes them;
- leaves the Nodes the flow created in place and lists them, since they belong
  to the Tenant. A Tenant Administrator deletes them if they are unwanted.

Each item is stopped on its own; the answer says for each Run, Remote Grant
(`remote`), Subnode and Duty whether it was `stopped`, `already_stopped` or
`failed` with the error, so one failure never keeps the rest running. A Remote
Grant that failed is revoked already; stopping the flow again asks its receiver
once more. Whoever may control a Run of the Root
Task (`run.control`) or administers the Root Task's Node stops it, from a
browser session; no Agent does.

## Nodes Agents create

An Agent creates a Node with `node_create` only when the person at the root
of its chain holds `node.create`. It deletes one with `node_delete` only when a
Run of its own flow created it (the creating Task and the deleting Run's Task
share a Root Task; decision 20), and likewise removes a Subnode with
`delete_subnode` only from a Run of the creating Agent in the creating flow
(decision 46). Another flow, even of the same person, does not, and the root
person's `node.delete` does not widen this. That flow's root person and Tenant
Administrators delete such Nodes and Subnodes from a browser session. See
[authorization](../authorization.md).

A Duty set up by a person starts each Task as its own Root Task, so the Task of
one tick cannot delete a Node the Task of an earlier tick created.
