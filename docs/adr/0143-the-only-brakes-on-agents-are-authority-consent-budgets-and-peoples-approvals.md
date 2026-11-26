# The only brakes on Agents are authority, consent, Budgets and people's approvals

Decided on 2026-11-18 in the agent activation grilling (umbrella Issue #1007). Aidash had grown many limits that keep Agents from multiplying: Agents could not create Nodes, Schedule-born work could not register Schedules (ADR 0134), Subnodes could not nest and were capped per Run and in members (ADR 0110), generation had agent, concurrency and depth caps and token budgets, delegation chains stopped at 32 links, a Remote Grant Run could not hand work on (ADR 0109), and every Run stopped at `max_steps`. The Tenant is meant to run like a company of agents that grows its own departments, teams and staff as its work needs.

## Decisions

- **Every limit whose only purpose is to stop growth is removed.** Agents may create Nodes, Duties, standing triggers, nested Subnodes without caps, and generated agents without count, concurrency or depth caps; delegation may run through any number of Nodes; `max_steps` is gone.
- **Four brakes remain.**
  1. _The requester's authority._ Everything an Agent creates acts as the root person of its chain and never exceeds that person. Each Task records its root person, so no chain walk is needed; the cycle check stays.
  2. _The other side's consent._ Another Node or Tenant decides whether it takes work, which tools that work may use, and whether such work may create Subnodes there.
  3. _Budgets_, optional per Tenant, Node, Duty and person, with a Soft Limit that mails chosen recipients and a Hard Limit that stops (default) or puts to sleep the work it covers. What an Agent creates draws on its creator's Budget.
  4. _Tenant approvals are made by people._ Except for the Tenant Configurator (ADR 0151), an Agent never makes a Catalog Approval, decides a Registration Request, invites, changes the Tenant's policy, groups, Approval Modes, Approval Rules, Tenant or Node Instructions, or raises a Budget, even when its root person is a Tenant Administrator. Approval Mode, Bypass included, only stops interrupting an Agent's actions.
- **Generated agents follow the same line.** A Generation policy may let the Agent write the instructions and pick components within bounds a person set. Approval Modes and Rules never approve a generated definition; only the policy's no-approval mark, set by a person holding `generation.manage`, skips the per-request approval.
- **Creating Nodes is a permission of the Tenant's policy**, held by Tenant Administrators at first and grantable to members; an Agent has it only through its root person. `subnode.create` becomes an ordinary member permission.
- **People can see and stop what Agents built.** An org chart shows what each chain created, under whose authority and with what spend, and one action stops everything a chain created. The stop pauses the flow's Duties and checks for Runs that committed while the pause waited for a Duty to start.

## Considered Options

- **Keep count caps on wakeups and agent-to-agent exchanges.** Rejected after Project Vend phase two showed agents chatting all night: a count cap stops useful growth as surely as waste, while a Budget stops waste by its cost.
- **Let Bypass approve generated definitions.** Rejected: it lets an Agent's chain write a Catalog Approval that only an authorised person may make.
- **Make Budgets mandatory.** Rejected: a Tenant may choose to rely on authority and consent alone.

## Consequences

- With no Budget set, an Agent's chain can spend without limit within its root person's authority.
- Agent-created Nodes belong to the Tenant and survive their creator; their Duties stop with their requester.
- Aidash refuses to store its own Subject Credentials as Tenant Secrets, so a Tenant tool cannot become a path to Aidash's administration API.

## Amendment (2026-11-19, implementation of Budgets, #1012)

Lead's calls made while building Budgets:

- **Spend is a ledger, not a sum over usage.** Usage records go with their Runs, and deleting a Duty, a Thread or a Node deletes Runs. The ledger keeps one Tenant-owned row per UTC day, Node, root person and Duty, written when a model call's usage completes, so removing work never gives money back.
- **A Task records its Duty once**, when the Duty starts it or a Run of a Duty's Task creates it, so a Duty's spend is charged without walking Task trees.
- **Only reported cost counts.** A call whose cost no provider reported is counted as an unpriced call and shown, but it cannot reach a limit. Model definitions carry no price list to estimate from.
- **Every model call passes the gate**: main steps, auxiliary calls and MCP sampling record a usage dispatch first, and memory-role calls and memory embeddings pass the same gate and post to the same ledger. A refused dispatch writes no usage record. Memory work outside any Run is charged to its Tenant and its Thread's Node.
- **What an Agent creates is carved out of its creator's Budget.** A Duty a Duty's Run sets up, and a Node a Run on a Node creates, record every Duty or Node above them in a Tenant-owned lineage written once at creation. A Duty or Node Budget covers and counts everything below it; the lineage outlives the Duties and Nodes it names, so deleting one gives no money back.
- **Generation token budgets are optional** (decision 41): without them a generated agent has no token limit and money Budgets limit its spend.
- **An Agent deletes only what its own flow created**: `node_delete` and `delete_subnode` compare Root Tasks, not root people, and the root person's `node.delete` does not widen this.
- **Stop pauses the Run like any pause a person lifts** (event `execution.budget_exhausted`). **Sleep is an Outage Wait** that rechecks every hour with no deadline, so a reset window or a raised limit lets the same step run. Neither spends a recovery attempt.
- **A Duty starts no Task** while a Budget that covers its Node, its Requester or itself is at its Hard Limit; the tick or event is skipped with reason `budget: <id>`.
- **A Soft Limit records one notice per window** and mails its recipients through the Deployment's SMTP settings. Without SMTP, the pass records the notice as skipped. A refused or interrupted delivery stays pending. If SMTP accepts the message and the replica stops before recording success, a later attempt can deliver it again.
- **In-flight calls can overshoot**: the gate reads spend recorded so far, and a call already dispatched completes. A call a newer lease abandoned still posts its cost when it finishes.

## Amendment (2026-11-24, #1117)

ADR 0146: every other limit, on threads, connections, counts, pages or waits, names what it protects, counts at that scope and says so when hit, or is removed.
