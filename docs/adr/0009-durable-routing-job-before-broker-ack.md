---
status: accepted
---

# Acknowledge broker delivery after a resumable routing job is committed

For [Issue #70](https://github.com/kent8192/aidash/issues/70), ACK JetStream only after committing a unique PostgreSQL routing job that can finish recipient enumeration and resume after a crash. Persist recipient batches and their cursor atomically, then perform bounded semantic selection and atomic execution handoff per recipient; the broker does not wait for inference or Agent completion. This transfers an executable obligation to the authoritative database rather than an inert inbox marker, trading an additional durable stage for bounded fanout transactions and independent recovery of slow or failing recipients.

The [event-to-execution contract](../design/2026-09-28-event-routing-contract.md) distinguishes broker acceptance, selection, handoff and processing completion, and requires healthy-path tests with periodic recovery scanning disabled.
