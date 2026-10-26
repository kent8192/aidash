---
status: accepted
---

# A Catalog Approval Request grants its requester nothing

A person without `catalog.approve` who wants to use a definition that is not yet approved on a
Node had no path in Aidash: they saw only a badge. Approving on registration, preapproval policies
and exceptions for trusted sources would shorten that path, but they change the Tenant's safety
model, so #440 leaves them out. We decided that such a person makes a Catalog Approval Request.
It names one definition and one Node, carries a reason, and pins a digest of the definition's
transitive closure. It is shown to everyone who may approve any part of it. An approver approves
through an Approval Review or declines with a reason. The request moves to fulfilled when
everything it names is approved, and to stale when the closure no longer matches. A stale request
cannot be approved. Making, repeating or withdrawing a request never confers authority, and the
requester sees only what they are allowed to see.

## Considered Options

- **A message telling the person to contact an administrator.** That leaves no actionable next
  step and no record. Rejected.
- **A requested approval that auto-applies after a delay or a quorum.** This is a preapproval
  policy in disguise. Rejected.
- **External notifications (email, chat) in the same change.** Deferred to a separate Issue.
  The in-app pending list is enough for the first-task path.

## Consequences

- Each requester has at most one open request per definition and Node, and requests do not expire.
- Run-time approvals (ADR 0075/0076) remain separate: a request concerns only installation-time
  Catalog Approval.
