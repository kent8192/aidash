# A Run renders its Context Projection under one immutable Projection Version, fixed in its Binding snapshot

Decided on 2026-10-09 (#116, #172, #178). A Projection Version is the rule for rendering a Context Projection (ADR 0010-staged-context-recovery) into the bytes of a model request. There is one axis:

- `Legacy=1`: today's single JSON user message.
- `Ordered=2`: one user message ordered by volatility, rendered as text parts: the Run context (identity, task, reference documents, run-message summary, Execution Summary), then one part per history event from oldest to newest, then the current step state. Its system prompt starts with the Tenant cache salt (#172). Every route receives the same parts, whether or not it caches.
- `Native=3`: model-native assistant and tool messages (#178).

Each value is a complete, frozen rendering; layout and message format are never combined at runtime. An Agent definition names its Projection Version. The version is copied into the Run's Binding snapshot and fixed for the Run, in the same way as the Exposure policy (#115) and the Context Policy (#113). It is not part of `StateVersion`, because `StateVersion` versions persisted state, while a Projection Version versions what the model sees. Model definitions declare the versions they support, and Agent registration rejects an unsupported choice. Definitions spell the versions `legacy`, `ordered` and `native` (`projection_version` on the Agent, `projection_versions` on the model); the numbers above only order them. Both fields are omitted when absent, so existing definitions and snapshots keep their bytes.

## Considered Options

- Extend `StateVersion`. Rejected: prompt changes would then require state migrations.
- Separate layout and message-format fields. Rejected: they multiply into combinations that each need golden fixtures and cache reasoning, while only three renderings are wanted.
- A node-wide default with per-Agent overrides, or a Tenant policy. Rejected: Agent definitions are already immutable and pinned per Run, and #123 requires new behavior to be versioned and deliberately enabled.
- A dedicated column on the Run. Rejected in favor of the Binding snapshot, where the other versioned per-Run policies already live.
- Upgrading in-flight Runs. Rejected: Runs recorded before `Native` lack Model Turns, and inventing turns or reasoning is forbidden.
- `Ordered` as a single JSON string. Rejected on 2026-10-09 (#142): a breakpoint cannot sit inside a string, so explicit caching would stop at the end of `system` and the history would not be reused until `Native`.
- `Ordered` as two text parts, Stable Prefix then volatile. Rejected on 2026-10-10 while implementing #172: an explicit cache entry is read only at a content-block boundary that an earlier request marked, and a single Stable Prefix part grows with every step, so the previous boundary never recurs. One part per history event keeps every earlier boundary in place.

## Consequences

- Binding snapshots without the field decode as `Legacy` and must keep producing byte-identical requests. Recovery and resumption reproduce the pinned version.
- Only `Native` Runs append `ContextEvent::Assistant` to the Context Journal. `Legacy` and `Ordered` projections never see it.
- Under `Native`, pruning and the Summary Stage treat a Model Turn as a unit. An elided tool result keeps its `tool_call_id`, and no request contains a partial Model Turn.
- `Native` does not persist or replay reasoning output.
- Unsupported versions fail at Agent registration and again in the adapter. There is never a silent fallback to `Legacy`, and ZDR stays enforced.
- The default for new Agent definitions stays `Legacy` until the matched-task comparison in #181 supports a change.

## Amendment (2026-10-30)

New Agent definitions that omit `projection_version` are registered as `Native` when their model declares Native, together with the `structured@3` Harness Contract (#532, ADR 0093 amendment). The value is written into the definition at registration. A definition or Binding snapshot read without the field still decodes as `Legacy`, so pinned Runs keep byte-identical requests. Agents whose model does not declare Native keep the Legacy default.
