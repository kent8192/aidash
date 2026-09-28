---
status: accepted
---

# Give Harness workers a dedicated activation consumer

Use a dedicated node-scoped JetStream stream for Run activation and one durable pull consumer shared by interchangeable Harness workers in that Node's execution-capability profile. Only worker and combined modes consume these notifications. Run the recoverable activation publisher in every mode, so worker-originated transitions can activate other workers while the HTTP server is unavailable.

This adds broker objects and configuration but prevents a server-only event consumer from acknowledging the only notification needed by an independent worker. Domain-event routing keeps its own consumer and durable recipient handoff under #70; its role placement remains that Issue's responsibility. Combined deployments use the same durable activation contract as split deployments.

## Considered options

- Share the existing domain-event consumer with server and worker processes. A server can take the delivery without any worker receiving it, and routing and execution compete for the same delivery ownership.
- Give every worker an independent subscription. All replicas receive the same activations, increasing duplicate reads and contention, with consumer lifecycle tied to replica churn.
- Share a worker-only durable activation consumer. Chosen for explicit ownership and distribution across equivalent replicas, while existing database leases and fencing remain authoritative.

Use WorkQueue retention with explicit ACK, initial configurable limits of 24 hours and 1 GiB, and DiscardNew on capacity exhaustion (Q12). This trades broker-side history for bounded transport storage; durable Run/input/effect records and unresolved activation obligations remain in PostgreSQL. Broker retention or publication confirmation cannot delete execution responsibility.

Accepted by the user on 2026-09-28 (Q4 and Q12). Startup/reconnect reconciliation and bounded database recovery remain required (Q6); broker unavailability cannot block HTTP or worker startup. [ADR 0006](0006-acknowledge-durable-activation-dispositions.md) records the ACK boundary and capacity rule. The [consolidated specification](../design/2026-09-28-worker-activation-specification.md) defines identity, provisioning and lifecycle guidance.
