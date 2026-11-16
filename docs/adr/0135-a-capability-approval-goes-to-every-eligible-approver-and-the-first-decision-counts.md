# A capability approval goes to every Eligible approver, and the first decision counts

Decided on 2026-11-13 for #299. A pending capability approval (an outbound request, a Web disclosure, a Tool call approval or a Sampling approval) was addressed to one Designated approver: the Run's requester when the Tenant's policy let them approve, otherwise the eligible person whose subject ID sorted first. When that person was away, the Run waited until the approval expired (24 hours by default), a repeated request picked the same person again, and the other people who could approve never saw it.

## Decisions

- **When the requester can approve, they alone decide.** ADR 0127 stands: the approval is offered to them inline in the Thread and in their approvals list, and their request never counts as the decision (ADR 0037).
- **Otherwise every Eligible approver is offered it, and the first decision counts.** A decision names the revision it saw; a later decision on the same approval is refused and told who decided and how. An expired approval is refused with a different message.
- **Who is eligible is judged when the approval is shown or decided, not when it is requested.** The record keeps who decided, not whom it was addressed to. A person who gains the right while it waits sees it; one who loses it no longer does. This also decides between the requester alone and everyone: if the requester gains or loses the right while it waits, the approval moves accordingly.
- **The Thread does not name who may approve.** While it waits it says that it is waiting for an eligible approver; once decided it names who allowed or denied it. Naming the candidates would show the requester part of the Tenant's policy.
- **More people may revoke a Run grant.** Besides the requester and the person who gave it, anyone currently eligible to approve that kind of request for that destination may revoke it. A Run grant still stops applying when the person who gave it can no longer approve.
- **Being offered an approval is the notification.** Eligible approvers see it in their approvals list and in-app notices; it leaves every list once decided. Mail is a separate Issue.

## Considered Options

- **Escalate to the next eligible person after a delay.** Adds a timing setting and keeps part of the wait. Rejected.
- **Let another eligible person claim the approval.** Nobody else sees it, so nobody would know to claim it. Rejected.
- **Address everyone even when the requester can approve.** Members may approve their own Runs' Tool call approvals under the initial policy, so every member's tool calls would fill the lists of everyone holding `capability_approver`. Rejected.
- **Fix the set of approvers when the approval is requested.** Keeps people who have lost the right and misses people who gained it, while eligibility was already checked again on every read and decision. Rejected.

## Consequences

- "Designated approver" is replaced by Eligible approver.
- Federated Runs keep asking the requesting Node through a human request (ADR 0075); this ADR does not change them.
- A Run whose requester can approve but is not watching, such as a Schedule's Run, still waits for that requester; widening it to everyone after a delay would be a separate decision.
- `AllowAlways` stays limited to a requester deciding their own approval, so another approver never creates an Approval Rule for someone else.
- ADRs 0037, 0075 and 0127 carry a note pointing here.
