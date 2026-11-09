# Statements have one time limit and a client deadline behind it

Amended by ADR 0124 (2026-11-09): temporary database failures in vector lookups wait without consuming a Run attempt.
Decided on 2026-11-05 in the #728 grilling. Server and worker connections set `statement_timeout`, 5 minutes by default, and nothing else that limits a session. A wrapper around Reinhardt's `DatabaseBackend` and `TransactionExecutor` adds a client-side deadline 30 s later, which fires only when the server does not answer. A statement that hits either limit is a temporary database error: the Run waits and resumes, and it counts towards no failure limit.

The two limits answer different failures. A live server enforces `statement_timeout`, including time spent waiting for locks. A server that vanished cannot, and SQLx sets no TCP keepalive, so without the deadline a query can wait for that reply for ever.

Each connection also asks Postgres to notice a vanished client within about 30 s (`tcp_keepalives_*`, `tcp_user_timeout`), so a dead backend's row locks do not last for the operating system's default of hours. `after_release` has a 5 s limit, because SQLx gives returning a connection no limit and a dead connection would keep its pool slot.

## Exempt

These run with neither limit, because their length comes from the design:

- The `atomic_gate` writer. It waits behind every read lease, and a lease can last as long as a model call, up to an hour.
- Deletes with no row limit: Tenant purge rows and audit history, `core_blob_references`, Node purge rows and `memory_history`. #744 splits them into batches and removes this exemption.

`aidash migrate`, maintenance commands, host commands (`issue-subject-credential`, Secret Store recovery) and the Observer have no limit either. A stall in any of these is seen only through `aidash_background_service_pass_age_seconds`, and only if it runs inside a background loop.

## Considered Options

- **`lock_timeout`.** Rejected: every authenticated request takes row locks, and a lock timeout in a transaction participant can abort a scoped transaction.
- **`idle_in_transaction_session_timeout`.** Rejected: read leases idle on purpose through inference and through whole HTTP handlers, and a killed lease loses its gate lock with no error.
- **A fork of SQLx with TCP keepalive.** Rejected by the owner: it would detect a vanished server in about 30 s rather than 5 min 30 s, but every SQLx update would have to carry the patch.
- **A proxy beside each Pod that holds the keepalive.** Rejected: one more part in Compose, Helm and every Self-hosted install, and TLS host name checks would see the proxy.
- **A time limit on one pass of a background loop.** Rejected: a pass can hold a Runner call of up to 1900 s and has no limit on rows, so no limit is safe.

## Consequences

- A new default method on either Reinhardt trait bypasses the wrapper without a compile error, so check `server/src/database/limits.rs` on each Reinhardt update.
- Enterprise auth and marketplace set the same limit and keepalives but have no client deadline, because they do not use the wrapper. Both limit `after_release` to 5 s.
- A connection lost during `COMMIT` leaves the outcome unknown, as any lost connection does.
