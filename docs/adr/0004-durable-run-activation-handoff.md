---
status: accepted
---

# Persist the Run activation obligation with runnable state

[Issue #70](https://github.com/kent8192/aidash/issues/70) owns logical recipient selection and durable Run/input handoff; [Issue #71](https://github.com/kent8192/aidash/issues/71) owns activation of independent Harness workers. Persist the obligation to notify workers atomically with the update that makes a Run runnable, including direct Run admission. This separates recipient decisions from execution placement and avoids a committed Run being stranded by a crash before a transient notification is sent.

PostgreSQL remains authoritative for Run state, execution ownership and effect history. Activation transport must preserve existing authorization, distributed-transaction visibility and unsafe-effect reconciliation; delivery cannot grant permission or justify replaying an uncertain effect. Within this Issue, interchangeable workers serve one executing Node and execution-capability profile. Specialized capability/tenant pool placement is a separate decision, and remote admission/Home-command authority remains owned by #38.

## Considered options

- Forward domain events directly to workers and repeat recipient selection there. This couples #71 to #70's subscription policy and duplicates responsibility for creating or resuming Runs.
- Commit runnable state, then send only an in-memory or best-effort notification. A crash between these steps leaves periodic recovery as the normal way to discover accepted work.
- Persist runnable state and its activation obligation together. Chosen because it gives producers and workers a recoverable handoff while leaving transport and consumer ownership explicit.

The user accepted this boundary on 2026-09-28 (Q1-Q2). [ADR 0005](0005-worker-owned-activation-consumer.md) records consumer topology; [ADR 0006](0006-acknowledge-durable-activation-dispositions.md) records notification identity and the durable ACK boundary. The [consolidated specification](../design/2026-09-28-worker-activation-specification.md) contains accepted recovery/operational policies and the acceptance plan.
