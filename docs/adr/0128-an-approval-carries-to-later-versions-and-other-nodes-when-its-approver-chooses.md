---
status: accepted
---

# An approval carries to later versions and other Nodes when its approver chooses

Decided on 2026-11-11 for #873. A Catalog Approval let one exact version be used on one Node (ADR
0029). Every new version of an Agent, a dependency or the built-in compaction Decider asked for a
new approval, an Agent's dependencies needed their restrictions widened again (ADR 0095), and a
Tenant with several Nodes on one Deployment repeated each approval per Node, though those Nodes
share the Tenant's Data Protection Level, Provider Credentials and the Deployment's runner.
Requests were seen only in the dashboard (ADR 0096). Teams iterating on Agents spent most of
their approval effort on content whose consent had not changed.

We decided that an approver may choose, with each Catalog Approval, two things:

- **Approval Scope**: this Node, or every Node of the Tenant on this Deployment, including Nodes
  created later. The second is the default. It needs `catalog.approve` on the definition on every
  current Node in the scope. A Node on another Deployment always approves on its own.
- **Version Carry-over**, on by default: a later version of the same definition is approved too,
  as long as what the approval consented to is unchanged.

What an approval consents to is what the version may use, where it sends content and how long it
is kept there. Per kind, carry-over stops when:

- an Agent gains a dependency identifier it did not use, widens `core_capabilities`, an `allow_*`
  flag or `agent_plugins`, or switches `remove_default`. Its instructions, description, name and
  tags, and the version of a dependency it already used, are not consented to;
- a model changes `provider`, the `endpoint` host, or its `provider_credential` or
  `plan_connection`. `model_id`, `modalities` and `wire_api` are not consented to, because the
  Tenant's Data Protection Level is enforced on each call;
- an HTTP or MCP tool changes the `endpoint` host or `secret`, adds `scopes`, or declares another
  operation Read-only (ADR 0078);
- a Decider adds a Route host, or enables or lengthens `state_retention`;
- any dependency of the version is not approved on that Node. A Carried Approval counts.

A **Carried Approval** is written when the version is registered or installed, as an ordinary
approval row on every Node of the scope, recording the approval it came from and its approver. It
carries from the highest lower semver version whose approval chose carry-over, so a backport 2.0.1
carries from 2.0.0 and never from 2.1.0. The carried Agent version's subject joins the groups of
the version it came from, and every restriction that named that version widens to name the new
one too, as a new revision attributed to carry-over. A version that does not carry tells its
registrant which items changed and offers a Catalog Approval Request.

Catalog Approval Requests now notify: every dashboard page shows the number of pending requests,
and a Deployment with SMTP configured mails each request once to the verified address of everyone
who may approve any part of it. The mail names the definition, the Node, the reason and a link to
the review, nothing of the definition's content. Each person can turn that mail off.

## Considered Options

- **No carry-over, a one-click re-approval showing the difference.** It keeps every decision with
  a person but removes no step. Kept only as what a version that does not carry shows.
- **Automatic carry-over for every approval.** Anyone holding `registry.create` could change what
  an approved Agent does without an approver, which is why ADR 0062 rejected inheriting groups.
  Rejected; the approver's choice is what makes the inheritance safe.
- **Deciding carry-over at use rather than writing rows.** Withdrawal, history, the Use Recheck
  (ADR 0122) and the approval list would each need a case for a version usable without a row.
  Rejected.
- **Withdrawing an approval also withdraws what was carried from it.** Withdrawing the old version
  is the ordinary step of upgrading an Agent (ADR 0122), and it would stop the new one. Rejected;
  the withdrawal dialog offers it as a choice.
- **Approvals per Tenant with no per-Node choice.** It removes trying a version on one Node first.
  Rejected.
- **Built-in Deciders and Sandboxed tools approved when a Tenant is created.** That removes the
  Tenant's decision to send content to their hosts (ADR 0102) or act outside Aidash (ADR 0034).
  Rejected; with Approval Scope and carry-over that decision is made once.
- **Chat or webhook notices.** Each needs a per-Tenant connection. Rejected for now.

## Consequences

- Existing approvals keep carry-over off and the scope "this Node". Demo data approves the
  compaction Decider and its Agents with carry-over and the Deployment-wide scope.
- Marketplace copies, and built-in definitions that a Deployment upgrade versions up, follow the
  same rules.
- A carry is shown in the approval list and its history, never notified.
- The withdrawal dialog offers to withdraw the versions carried from an approval and to stop
  carrying it further. A Carried Approval is otherwise independent of its source.
- A Personal Tenant still approves Sandboxed tools once (ADR 0034); there is no exception for it.
- Without SMTP there is only the badge, and startup is never refused for it.
