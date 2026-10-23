# ChatGPT Native turns preserve messages and tool calls without retaining provider reasoning

Decided on 2026-10-23 for #135 and #249. ChatGPT plan models support the Native
Projection Version by mapping its messages, tool calls and tool results to
Responses input items. They do not retain or replay encrypted provider reasoning.
The Context Journal continues to record the observable conversation and tool
history without reasoning output. This keeps recovery, compaction and disclosure
independent of opaque, provider-bound state whose availability for plan usage has
not been established.

## Considered Options

- **Persist and replay encrypted reasoning.** This could improve reasoning
  continuity and caching, but requires a provider-bound journal format, storage
  and request limits, retention and compaction rules, and live confirmation that
  plan usage returns replayable reasoning. Rejected for this delivery.
- **Support Native messages and tool items without reasoning retention.** Chosen.
  It meets #249's Native requirement and its explicit option to reject reasoning
  retention with reasons, while preserving the existing journal policy.

## Consequences

- Responses calls keep `store: false` and `stream: true`. They send the projected
  history and do not rely on `previous_response_id` or server-stored history.
- Reasoning items are neither written to the Context Journal nor exposed in
  Inference Progress, events, logs, traces or API responses. Provider reasoning
  token counts may still contribute to usage accounting.
- Stable Prefix and Request Estimates describe the actual serialized request;
  there is no hidden reasoning payload to retain or estimate.
- Reasoning continuity and cache benefits from encrypted reasoning replay are
  not promised. Adding replay later requires revisiting this decision and the
  journal's retention and recovery contract.
