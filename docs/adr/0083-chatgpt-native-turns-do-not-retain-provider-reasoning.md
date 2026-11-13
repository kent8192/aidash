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
  there is no hidden reasoning payload to retain or estimate. Native plan
  requests count Responses instructions, input items and namespaced tools.
  Legacy and Ordered requests keep their existing accounting, and Chat request
  identities stay unchanged.
- Reasoning continuity and cache benefits from encrypted reasoning replay are
  not promised. Adding replay later requires revisiting this decision and the
  journal's retention and recovery contract.

## Amendment (2026-11-13, #942)

The premise of this decision changed with #942. Current provider documents say
that resending previous reasoning keeps reasoning continuity and lets the
previous output become cached prefix: DeepSeek rejects a tool conversation
without it, Gemini 3 and 4 require thought signatures on function calls, and
OpenAI, xAI, Kimi, MiniMax, GLM and Mistral recommend it. Native Runs now record
the reasoning of each accepted response on its `assistant` event exactly as
received: Chat `reasoning_details` (or plain reasoning text when that is all a
route returns) and Responses `reasoning` items, encrypted content included.
Every later request resends it unchanged in the transport's format, on ChatGPT
plan routes as well. A model's `reasoning_passback: false` turns a route off.

- Recorded reasoning is treated like the model's own text under ADR 0046: the
  Context Journal keeps it after a revocation, and it is deleted with the
  Journal row. It is left out of the Journal export that Journal Query, journal
  search and reads, and memory extraction use.
- Unlike the model's text, a request does not resend the reasoning of a turn
  recorded at or after a result that was revoked or expired, because reasoning
  often copies evidence verbatim and encrypted reasoning cannot be inspected.
  Routes that require the field get an empty reasoning string instead. The
  decision is stored on the Run's context, so a summary that absorbs the
  placeholder does not bring the reasoning back.
- Request Estimates count the resent reasoning. Compaction steps drop the
  reasoning of turns older than the protected recent tail; ordinary steps
  never drop any.
- Reasoning is still never shown in Inference Progress.
