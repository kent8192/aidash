# A requester who can approve decides once, where they are

Amended by ADR 0135 (2026-11-13): "Designated approver" below now reads Eligible approver; when the requester cannot approve, every Eligible approver is offered the approval.

Decided on 2026-11-11 for #874 (part of #872). Several rules exist so that a requester cannot stand in for a separate approver. They also applied when the requester and the approver are the same person, who then decided twice, often somewhere other than where they were working. The rules stay; what changes is where and how often that person is asked.

## Decisions

- **A request is still never an approval.** ADR 0037 holds: pasting a URL, or asking a Run to do something, does not approve what it leads to, because the Run's context, not the request, decides whether the disclosure or call is safe. Instead, when the Designated approver of a pending capability approval (a Web disclosure, an outbound request or a Tool call approval) is also the person at the root of the Run's delegation chain, the approval is offered inline in the Thread where they watch the Run, with the same choices and the same record as in the approvals list. Nobody else sees it there, and the approvals list keeps offering it.
- **ADR 0075 stands.** A person watching their own Run who is not a Designated approver is so because the Tenant's policy says so; the initial policy lets members approve their own Runs' Tool call approvals.
- **An Invitation may join directly.** When issuing an Invitation, its issuer may choose that opening it joins directly, and that choice starts on. Only an issuer who may also approve Registration Requests for that Tenant can choose it. On redemption Aidash checks again that the issuer may still approve; if so, the Registration Request is approved at once, recorded as approved by the issuer, with the Invitation's proposed groups, and the Tenant Administrators are notified. If not, the request waits for an administrator as before. The link stays single-use and expires within 7 days.
- **A decline can cover the rest of the Run.** When declining a paused call (ADR 0118), a person may also decline that tool's later calls in the same Run that meet the same kind of refusal. Those calls get the same fixed call error without pausing. It never reaches other Runs, child Runs included, and gives the Run no authority. The same people who may decline the call may choose it.
- **A Personal Tenant's owner may use Bypass.** A Personal Tenant's initial policy names `approval.mode.bypass` for its owner. ADR 0076 kept Bypass off because a Tenant answers for what its members' Runs do; in a Personal Tenant the owner is the Tenant. The recorded acceptance is still required. Other Tenants are unchanged.
- **A Workbench test asks the person who started it.** Instead of refusing every approval, a Workbench test offers it, in the Workbench, to the person who started the test when the Tenant's policy lets them approve it. They may allow once or deny; no Run grant, and their Approval Modes and Approval Rules do not apply, so a test shows every approval a real Run would meet. Otherwise it is refused as before.

## Considered Options

- **Treat the request as the approval** (PR #99). Rejected again for ADR 0037's reason.
- **Let the Invitation's issuer approve by default without a recheck at redemption.** An issuer who has since lost the right would still admit people. Rejected.
- **Decline a tool for every Run.** That is a change of policy, which Tenant Administrators make through the policy or a person through a deny Approval Rule. Rejected.
- **Make Bypass available by default in every Tenant.** Rejected for ADR 0076's reason.
- **Apply Approval Modes and Rules to Workbench tests.** A test would hide the approvals it exists to show. Rejected.

## Consequences

- Federated Runs keep asking the requesting Node through a human request (ADR 0075). Building capability approvals there is a separate Issue, as are evaluation fixtures, which have no person to ask.
- A Schedule whose Registrant leaves stops until another person takes it over; this ADR decides nothing about it.
- ADRs 0024, 0037, 0076 and 0118 carry a note pointing here.
