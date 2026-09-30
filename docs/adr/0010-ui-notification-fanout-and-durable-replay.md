---
status: accepted
---

# Fan out UI notifications independently and replay from PostgreSQL

For [Issue #72](https://github.com/kent8192/aidash/issues/72), each API process uses an independent ordinary Core NATS subscription to the existing Node event subject, retaining PostgreSQL Outbox and JetStream publication while keeping execution consumers separate. PostgreSQL remains the authority for ordered, currently authorized UI event data and reconnect replay; notification receipt only prompts a read and cannot advance a browser cursor or authorize delivery. We accept recovery latency for missed at-most-once hints through startup, reconnect and bounded reconciliation, avoiding per-replica JetStream consumer lifecycle and a second UI delivery ledger; the accepted intervals and verification conditions are recorded in the [design interview](../design/2026-09-30-sse-event-delivery.md).
