---
status: accepted
---

# Acknowledge durable activation dispositions before Run completion

An activation consumer ACKs only after committing a valid Run lease or a durable disposition that proves the activation is satisfied, no longer actionable, or remains owed under an explicit future trigger or retry obligation. Receiving a message or invoking local Notify does not establish that boundary. Model/tool completion is governed by the Run journal and effect contract rather than the lifetime of a broker delivery.

Use a versioned reference envelope identifying the executing Node, Run, activation and activation generation. Generations are distinct from fencing revisions and observed-input positions. Reload current state before a conditional claim, and preserve newer activation obligations when settling an older delivery. Another worker's valid lease cannot be treated as proof that new input was observed. Database commit uncertainty leaves the message unacknowledged for safe reconciliation/redelivery.

This adds durable handoff state but avoids holding broker messages across long or uncertain external operations. ACK loss may redeliver a reference; database ownership, generations and existing effect reconciliation prevent it from creating permission or an unconditional replay. Due-time obligations also live in PostgreSQL and feed this same path, allowing a broker restart to leave scheduling intent intact.

## Considered options

- ACK on receipt or on a process-local wakeup. A crash can remove the only useful notification without transferring execution responsibility.
- ACK only when the Run finishes. Long inference and tool operations become coupled to broker delivery timers without improving the safety of uncertain effects.
- ACK a committed execution handoff or explicit durable disposition. Chosen to make the notification boundary short and recoverable while retaining the existing durable execution contract.

Q11 preserves pending generations independently of observed input and re-arms eligible work at lease release or expiry. An intentional later notification advances a durable publication epoch; retrying an uncertain publication reuses the epoch. This prevents broker deduplication from suppressing a required re-notification while retaining database-level protection against duplicate execution.

Accepted by the user on 2026-09-28 (Q7-Q8 and Q10-Q11). Capacity follows Q9: reserve a free execution slot before pulling, with no speculative message backlog. The [consolidated specification](../design/2026-09-28-worker-activation-specification.md) defines dispositions, invalid-message handling and operational/failure acceptance.
