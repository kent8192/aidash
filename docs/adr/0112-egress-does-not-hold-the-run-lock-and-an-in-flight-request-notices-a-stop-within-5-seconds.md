# Egress does not hold the Run lock, and an in-flight request notices a stop within 5 seconds

Decided on 2026-11-04 for #715. Egress admission and metering read Run control without locking the Run row. Admission locks the operation's total budget and then its service budget for the short transaction that updates them. It calls the Runner before taking either budget lock. Live preview sessions use their own budget lock. A request started after a stop commits is refused; an in-flight request checks its authority at least every five seconds and is cut when the Run stops. A request that passed its check immediately before a stop may still leave the gateway. When the authority cannot be reached, a request survives up to three failed checks because a stop cannot be committed during such an outage either.

## Considered Options

- **Keep the Run lock throughout admission and metering.** This serializes requests and prevents activation claims from acquiring the Run during heavy traffic. Rejected.
- **Check only when charging body bytes.** A stalled request or a small response may not charge within five seconds. Rejected.
- **Revoke every request synchronously when a Run stops.** This would require tracking and coordinating all in-flight requests across gateway instances. Rejected in favour of a bounded live check.

## Consequences

- Budget rows serialize request counts and byte debits without blocking activation on the Run row.
- Operation tokens and Live preview sessions retain a bounded stop guarantee, including while a stream is idle.
- The future Scratch Space External service admission must also avoid locking the Run row.
- Amends ADR 0054.
