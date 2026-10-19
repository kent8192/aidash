# A Tool Batch runs under one Run authority lease

Decided on 2026-10-09 (#119). Every call of a Tool Batch runs under the Run's single outer authority lease. That lease is the authorization transaction that holds its shared row locks for the whole step. Each call opens its own inner transaction from that lease. Authority checks and admission for every call finish before any call is dispatched. A revocation therefore waits until the slowest call in the batch has finished. The outer scope is a read/write lock. Only core tool dispatch takes the read side; every other user takes the write side.

## Considered Options

- Give each call its own authority scope and re-inherit grants per call, so a revocation can land between calls. Rejected: a call admitted under the old grant could still finish and have its result adopted after the revocation committed. Preventing that needs a recheck at adoption plus rules for discarding or keeping completed reads. Holding one lease removes the case entirely, as it already does for a single sequential call.
- Keep the outer scope behind an exclusive lock and serialize dispatch. Rejected: calls would never overlap, which defeats the batch.

## Consequences

- How long a revocation waits now depends on the slowest call in the batch, not on one call. That wait is bounded by existing tool timeouts and by the Run and process Tool Parallelism ceilings.
- Each concurrent call holds one data connection for its inner transaction. The per-process ceiling is capped at the runtime worker pool size minus the worker slots, and startup rejects a ceiling that leaves no pool headroom.
- Batched calls must not need the outer scope exclusively while they execute. Only operations whose execution uses an inner transaction can declare Concurrency safety. `memory_recall` borrows the outer scope mutably, so it stays Sequential until that changes.
