---
status: accepted
---

# Product sessions have local logout and online revocation checks

In the target shared authentication architecture, ordinary logout ends the current product's session on the current device without ending sessions in other Aidash products or devices. Explicit all-products logout ends authentication sessions across every Aidash product and device. Session Revocation does not stop work already admitted on the person's behalf, preserving ADR 0004's distinction between session revocation and Identity Disablement.

Each newly started protected operation checks current authentication validity with the Authentication Server. After an account stop has completed there, a protected operation started afterward is refused. A failed validity check does not fall back to a previously valid token or cached active response. This deliberately makes new authenticated operations depend on the Authentication Server's availability; an unexpired token alone cannot authorize them.

A short operation admitted before a stop or logout may finish, including committing its write after the stop or logout completes. The next operation must recheck. A Run is not one unbounded admission: its model and tool execution boundaries recheck separately. This replaces the current single-database ordering with an explicit operation-boundary guarantee rather than attempting a distributed transaction between authentication and each product.

Each protected stream frame is a separate admission boundary. Before admitting a frame for transmission, the product performs the Authentication Server online check and its own local authority check. A denial or unavailable Authentication Server closes the stream and discards buffered data not yet admitted for transmission. Reconnection from a cursor rechecks authority rather than inheriting the old connection's admission. Periodic checks on an idle connection may close it earlier, but do not replace checks for protected frames. An already-admitted frame follows the same in-flight rule as other short operations.

At a boundary that requires human-origin authentication, an unreachable Authentication Server pauses the Run with its state intact. Work paused solely by that transient outage may resume automatically after recovery only after checking the original External Identity, Mapping, credential and local permissions. A definite stop or withdrawn authority does not auto-resume, and another linked Identity is not substituted.

Interactive and durable-origin validity are distinct checks. An interactive operation checks its Product Session, Account and source External Identity as applicable. Previously admitted durable work checks its original Identity and origin, relevant Account Suspension and local authority without requiring the original browser or Desktop session to remain alive. Logout, session expiry, migration session cutoff or removal of a sign-in Link alone is not a reason to stop that work. Legacy work with an as-yet-unassigned Account still requires its source Identity and local authority checks under ADR 0088.

The Authentication Server retains the existing GCIP status freshness policy: a successful upstream check is usable for five minutes; if a subsequent lookup fails, validity may be used only until fifteen minutes after the last success. After that deadline, status-dependent operations are refused and work pauses at its next boundary. This is distinct from failure to reach the Authentication Server, which denies a new protected operation immediately. An explicit stop already known to the Authentication Server always overrides a previous positive upstream result.

## Considered Options

- **Ordinary logout signs the person out of every product.** Rejected: leaving one product would unexpectedly interrupt another. Cross-product logout remains available as an explicit operation.
- **Accept a token or cached active result until its local expiry.** Rejected for new protected operations: it would allow them to start after a completed account stop.
- **Preserve single-database lock ordering across the extracted service.** Rejected for the target architecture: it would turn every product mutation into a distributed coordination problem. Already-admitted short operations may finish; later boundaries must revalidate.
- **Fail or cancel durable Runs on a temporary authentication outage.** Rejected: pausing retains their work and allows narrowly authorized recovery.
- **Call GCIP for every product operation.** Rejected for the initial extraction: the existing five-minute freshness and fifteen-minute outage deadline remain, independently of online checks against the Authentication Server itself.

## Consequences

- A successful stop or logout does not promise that an earlier admitted write cannot still commit. It promises that a later protected operation cannot be admitted under the stopped authority.
- ADR 0086 fixes account-restoration authority, ADR 0087 fixes browser login, ADR 0088 fixes migration and ADR 0089 fixes internal RPC transport and service authentication. Per-frame remote checks make stream volume part of authentication capacity planning.
- This is an accepted target contract, not a claim that online revocation checks are implemented.
