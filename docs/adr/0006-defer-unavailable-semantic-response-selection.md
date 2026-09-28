---
status: accepted
---

# Preserve undecided recipient work when semantic selection is unavailable

For [Issue #70](https://github.com/kent8192/aidash/issues/70), apply deterministic recipient, authority, event-type and allow/deny filters before using Jev only where semantic response selection is necessary. A Jev timeout or failure defers that recipient's work with bounded retries and leaves an inspectable reason when attempts are exhausted, favoring visible delayed work over either silently dropping it or launching every eligible Agent without a response decision. Explicit requests that need no semantic selection bypass Jev while retaining all deterministic authorization and subscription checks.

The accepted initial defaults are at most three Jev attempts for one decision, ten seconds per attempt, and five- and thirty-second delays before the second and third attempts. Exhaustion requires an explicit retry; a valid semantic deferral is a different outcome and waits for a declared condition rather than repeatedly polling the model.

Eligible conditions are relevant new input/state revisions, a declared dependency reaching its required state, and authorized manual reconsideration. Coalesce condition changes, allow at most three automatic decision rounds including the initial round, and surface attention after 24 hours without a resolution; each round retains the transport-attempt bounds and shares the reaction-chain budget. Detailed state transitions are recorded in the [design](../design/2026-09-28-durable-agent-event-routing.md).
