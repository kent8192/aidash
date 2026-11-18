# A Duty acts only for its requester, and an event is input, never a requester

Decided on 2026-11-17 and 2026-11-18 in the agent activation grilling (umbrella Issue #1007). Until now an Agent started work only when a person wrote a goal, and Schedules (ADR 0134) were the only way to start work without a person present. The Tenant is meant to run like a company of agents: Agents that wake when GitHub CI finishes, five minutes before a calendar event, or when an email arrives, sleep in between to cost nothing, and sometimes do open-ended standing work such as looking for suitable people to contact.

## Decisions

- **A Duty is a standing responsibility in a Node that one Agent carries out for one person, its requester.** It holds a mission, a procedure (a checklist), Duty Triggers, an optional Router, an optional Budget, memory and its own Thread. It never completes. A Node holds any number of Duties.
- **A Duty has no authority of its own.** Every Task it starts acts for its requester, with the requester's Approval Mode, Approval Rules, Plan Connections and MCP OAuth Connections, exactly as ADR 0134 decided for Schedules. When the requester leaves the Tenant or loses access, the Duty stops until a person takes it over as their own action.
- **An event is input, never a requester.** A webhook body, a Watcher's finding, an email's sender or another Tenant's event is data the Agent reads. It cannot command the Agent, change the requester or grant authority.
- **Schedules become one kind of Duty Trigger.** The other kinds are a signed generic webhook, a Watcher (a script that polls an outside service), an event from a Node (within the Tenant by its policy; from another Tenant only through a registered Node), the completion of another Duty's Task, and a one-shot wakeup a Task books for itself.
- **Routing is deterministic.** An event that matches a wakeup a Task booked wakes that Task. Any other event goes to the Duty's Router, a script that drops it, acts on it, gives it to an open Task of the Duty or starts a new Task. A Duty without a Router starts one Task per event.
- **Scripts need no approval of their own.** A Router or Watcher runs in the Duty's Execution Pod under the requester's authority, reaching only what the Scratch Space and approved External services allow. The only condition is a Catalog Approval for shell or Python covering the Duty's Node. The Duty keeps every script version with its author and time.
- **A Duty never runs on another Tenant's Node.** A partner that should do standing work for us sets up its own Duty, and we send it events.

## Considered Options

- **Authority on the Duty itself, like a job position.** Rejected: a Duty would need a new kind of authority that no Plan Connection or Approval Rule reaches, and nobody to answer its approvals. Project Vend phase two (Anthropic, 2025-12-18) also showed an agent shop handing its "CEO" role to a staff member on an unproven claim of a vote; authority that sits on a role is what such a claim captures.
- **Human approval for every script version.** Rejected: an Agent can already write and run any code inside approved shell or Python, so approving a Router adds no protection, and a deterministic script is harder to talk into anything than a model.
- **Duties hosted on another Tenant's Node.** Rejected: the receiving Tenant would run standing work whose requester it cannot control or charge.

## Consequences

- A departed requester's Duties stay visible but stopped until a person takes them over or deletes them.
- Mail, Slack and other delivery are built inside a Duty with its procedure and tools; Aidash adds no notification channel of its own beyond Budget mail.
- Where webhooks cannot reach a Deployment, a Watcher polls instead; there is no inbound relay.
