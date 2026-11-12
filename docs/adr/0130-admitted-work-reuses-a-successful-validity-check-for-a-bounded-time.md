# Admitted work reuses a successful validity check for a bounded time

Decided on 2026-11-12 in #876. Supersedes ADR 0085's rule that each protected stream frame is a separate online admission boundary, and its rule that an unavailable Authentication Server closes the stream at once. ADR 0085's rejection of a cached active result for **new** protected operations stays.

A successful online validity check against the Authentication Server may be reused for a Deployment-wide time **T** before it is made again. T is set in the Deployment's file configuration, defaults to 30 seconds and ranges from 0 to 300 seconds. Neither a Tenant nor the dashboard can change it. With T = 0, every frame checks online as ADR 0085 decided.

The reuse applies only to work already admitted: protected stream frames, the periodic check on an open stream, and the boundaries of a running Run (bound-origin checks). A new HTTP request, a sign-in, a manual Run resume, an egress request and a preview admission still check online every time.

A reused result is keyed by what it proved: the External Identity and Product Session for an interactive check, and the External Identity, Account and recorded epochs for a bound origin. Every stream and Run with the same key shares it. A failed or refused check is never reused. A local logout drops its entry at once. The product's own authority checks still run on every frame and boundary.

When the Authentication Server stops answering, admitted work continues until T after the last success. Then a stream closes as before, and a Run pauses and resumes automatically as ADR 0085 decided.

## Considered Options

- **Keep one online check per frame.** Rejected: a short Authentication Server outage cuts every open Run stream. Stream volume also becomes authentication load. Meanwhile, an operation admitted before a stop may finish anyway, so the per-frame check adds little to the guarantee.
- **A fixed T.** Rejected at the user's choice: a Deployment that wants ADR 0085's exact behavior can set 0.
- **Have the Authentication Server push stops to products.** Deferred: it needs a delivery channel and replay that do not exist yet. A bounded T keeps the guarantee without one.
- **Continue until the GCIP outage deadline (15 minutes) during an outage.** Rejected: a stop recorded during the outage would reach open streams only after 15 minutes.

## Consequences

- The guarantee becomes: after an Account stop completes at the Authentication Server, no new protected operation is admitted, and open streams and running Runs stop within T.
- Ten open tabs of one session make one online check per T, not one per frame.
- The application port's "no positive-result cache" contract now has an exception: cached results for admitted-work checks within T.
