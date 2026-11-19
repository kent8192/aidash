# Egress does not hold the Run lock, and an in-flight request notices a stop within 5 seconds

Decided on 2026-11-04 for #715. Egress admission and metering read Run control and Operation state without locking either row. Reconciliation uses a non-key-update lock, allowing request-record foreign-key checks to proceed while preserving serialized Operation updates. Admission locks the operation's total budget and then its service budget for the short transaction that updates them. It calls the Runner before taking either budget lock. Live preview sessions use their own budget lock. A request started after a stop commits is refused; an in-flight request checks its authority at least every five seconds and is cut when the Run stops. A request that passed its check immediately before a stop may still leave the gateway. When the authority cannot be reached, a request survives up to three failed checks because a stop cannot be committed during such an outage either.

## Considered Options

- **Keep the Run lock throughout admission and metering.** This serializes requests and prevents activation claims from acquiring the Run during heavy traffic. Rejected.
- **Check only when charging body bytes.** A stalled request or a small response may not charge within five seconds. Rejected.
- **Revoke every request synchronously when a Run stops.** This would require tracking and coordinating all in-flight requests across gateway instances. Rejected in favour of a bounded live check.

## Consequences

- Budget rows serialize request counts and byte debits without blocking activation on the Run row.
- Operation tokens and Live preview sessions retain a bounded stop guarantee, including while a stream is idle.
- The future Scratch Space External service admission must also avoid locking the Run row.
- Amends ADR 0054.

## Amendment (2026-11-07, #758)

An in-flight request's check is the Continuation Check. It notices a stopped Run within five seconds, and a revoked read once the Read Recheck has withdrawn the command, so the bound for a read is 30 seconds, plus one pass of that loop, plus these five seconds (ADR 0116).

## Amendment (2026-11-16, #909)

A Host connection's byte debit follows the same rule as its check: a slow store is not a refusal. A debit the store does not answer, or answers with an error rather than a decision, holds the connection's bytes and is tried again; only three failed checks close the connection as unable to reach its authority. One debit waits at most 30 seconds, after which the connection closes as `egress_budget_wait_timeout`, so a debit stuck behind the Scratch Space's state lock is named rather than left to the five-minute idle timeout. A refund runs after the bytes have moved, so one that fails is reported and leaves the unused bytes counted for the day, but does not change how the connection ended. Debits stay paid before the bytes they cover, so a slow store never lets a connection exceed its Scratch Space egress allowance.

Considered: letting bytes move on credit while a debit waits, which keeps downloads moving but can exceed the allowance; and keeping the five-second debit deadline, which under load on 2026-11-11 closed working connections mid-download while their checks still had failures to spare, and marked finished ones as failed when their refund timed out.
