# Optional Workspace semantic context may degrade with a typed marker; required context fails closed

Decided on 2026-10-12 (#154) and refined on 2026-10-17 (#234). When retrieval of a local Run's Workspace semantic context stays unavailable after the bounded retry (five retries after 2, 4, 8, 16 and 32 s, the same as a required-Home Run), the step continues with Degraded context. This happens only when every Registry source feeding the workspace slot opts in. The opt-in is `on_unavailable: pause | degrade` on the `workspace_retrieval` source Binding and defaults to `pause`. In place of the context, the step gets the typed marker `{"status":"unavailable","cause":"embedding_provider"|"vector_backend"}`. Everything else fails closed with a typed pause: required-Home semantic context, native memory, a legacy `semantic_memory` source with no native provider (it has no opt-in), and every authorization, binding, contract, invalidation, budget or allowance failure. We degrade instead of pausing because an embedding outage should not stop a step whose inference is healthy and whose author has said the context is optional. Under #158 an alternate provider is impossible, and a Deployment-hosted model is never a failover target (ADR 0036).

## Considered Options

- **Put the opt-in on the Workspace's semantic index settings, next to `auto_context`.** Rejected: whether a step may go without context is decided by the Agent's author and fixed in the Run's Binding snapshot. It is not a property of the Workspace.
- **Wait for #110's ContextSource framework.** Rejected: the field sits on the existing `workspace_retrieval` variant, so no other adapter can carry it, and #110 can absorb it later.
- **Degrade when the opted-in source shares the slot with a legacy `semantic_memory` source.** Rejected: that would silently drop memory-derived entries.
- **Degrade when vector points are missing.** Rejected: missing points mean a damaged or unfinished index that waiting will not fix. The Run pauses so that someone repairs the index. Only transport failures, timeouts, HTTP 408/429/5xx and transient vector backend database failures degrade.
- **Repeat the full bounded retry on every step.** Rejected: during a long outage every step would wait about a minute. After one step of a Run degrades, its later steps try once and degrade at once on failure. A successful retrieval restores the full bounded retry.
- **Record the marker as a Context Journal event.** Rejected: successful retrievals are not Context Journal events either, so failures alone would appear there. The Run event `run.semantic_degraded` with the step and cause records it instead, next to `run.semantic_retrying` and `run.semantic_blocked`.

## Consequences

- The workspace and memory slots are retrieved independently. An unavailable workspace slot no longer stops memory retrieval, and a memory failure still pauses the Run.
- A successful workspace retrieval ends the previous outage even when native memory subsequently fails (#366). The execution port reports that progress separately; the executor persists the cleared outage and retry state under its current lease before passing the memory error to recovery, which reloads the stored Run. This applies to Legacy and Ordered Runs, including scoped authority.
- The marker is never stored as a Source observation, either across steps under a Retrieval Key (ADR 0020) or for recovery of the same inference boundary. The next attempt retrieves again.
- The model sees only the marker, as data, with no added explanatory text.
- Run inspection lists the degraded steps with their cause, and the dashboard marks a local Run that ran with Degraded context. Completion Gate and Completion Review are not affected.
- A required-Home Run pauses even if its Agent's source opts in. Admission accepts such a Binding without a warning, because the same Agent also runs locally. The Registry form offers `pause | degrade` as an explicit choice for `workspace_retrieval` sources and says that it applies only to local Runs.
- Degradation applies to local Runs with and without a scoped authority guard, on the Legacy and Ordered Projection Versions alike. A degraded step on the scoped path begins no dependency journal for the workspace slot.

## Incomplete indexing

The #342 decision (2026-10-23, ADR 0020) treats incomplete indexing during a Run as a typed, bounded wait. It is outside the embedding/vector transport outage cases that may degrade: even an opted-in source waits for the complete index and then pauses with `retries_exhausted` if necessary. A healthy embedding provider does not make an entry READY until indexing completes.
