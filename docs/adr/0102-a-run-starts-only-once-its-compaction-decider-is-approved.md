---
status: accepted
---

# A Run starts only once its compaction Decider is approved

Decided on 2026-10-27 for #540. ADR 0042 binds the built-in compaction Decider to every ordinary
Agent and requires each Node to approve it, and ADR 0084 keeps a typed pause for an unapproved
Decider. In practice nothing asked for that approval: the default Decider was not part of the
Agent's Approval Review and a Run started without it, so every ordinary Run on a fresh Node
worked until its context outgrew the window and then paused with `decider_unapproved`, often long
after anyone could tell why. We decided that the approval is required up front. A compaction
Decider that the Node binds to an Agent by default is part of that Agent version's Approval
Review, and the Agent cannot be approved while the Decider is not. Starting a Run of an Agent
whose bound compaction Decider, default or explicit, lacks its Catalog Approval on the executing
Node is refused before any Run exists, with a reason that names the Decider and leads to a
Catalog Approval Request (ADR 0096). Scheduled starts record that refusal as a failed start.

## Considered Options

- **Keep starting the Run and pause at the first oversized context (ADR 0084 as written).** The
  pause comes late, mid-task, and the person who started the Run usually cannot approve.
  Rejected.
- **Approve the built-in Decider automatically when a Tenant or Node is created.** Approving it
  is the Tenant's decision to send Run content to the hosts its Routes name (ADR 0042, ADR 0066);
  doing it implicitly removes that decision. Rejected.
- **A Node-wide readiness state that blocks every Run until some compaction Decider is
  approved.** It adds a new Node state and also blocks Agents that removed the default Binding
  and never compact through a Decider. Rejected in favour of the per-Agent check, which reuses
  the Approval Review and the dependency check that already exist.
- **Block withdrawal of the last approved compaction Decider.** A Tenant that wants to stop a
  disclosure must be able to do so at once, and an approval may be withdrawn but never turned
  into a standing refusal. Rejected; the withdrawal instead shows its impact first.

## Consequences

- The typed pause of ADR 0042/0084 remains for Runs already running when the approval is
  withdrawn, and for a Decider that is approved but unavailable (no Provider Credential Binding,
  call limit, provider outage).
- Agents that remove the default compaction Binding are unaffected. Generated-chain and
  RequiredHome Runs keep ADR 0042's runtime check, since they never receive the default Binding.
- For a Run executed on another Node, the executing Node's approval is what counts.
- Demo data approves the built-in Decider, so its Agents can start; the fixed-task evaluator's
  Agents remove the default Binding instead, because its deterministic tier has no route to call.
- Agent versions approved before this change have no recorded Decider approval; since Aidash is
  not deployed, they are approved again rather than migrated.
