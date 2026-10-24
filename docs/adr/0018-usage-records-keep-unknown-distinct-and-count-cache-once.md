# Usage Records keep unknown values distinct from zero and count cache tokens once

Decided on 2026-10-09 (#116, #177). Each Inference Attempt (ADR 0014-inference-progress) has one append-only Usage Record, keyed by the attempt's public identifier, never by the worker lease token. The record is inserted before provider I/O and completed at most once. Every field is optional and labeled by its source:

- provider-reported input, output, cache read, cache write and reasoning tokens;
- provider cost;
- the rejection class, such as Context Overflow;
- Aidash's Request Estimate, with the estimator's identity and confidence.

A value the provider did not report is unknown and is never stored or shown as zero. Quota settlement charges `prompt_tokens + completion_tokens`. Cache read and write counts are breakdowns of `prompt_tokens` and are never added on top. Settlement uses only complete usage. Incomplete, interrupted or uncertain attempts stay fully charged.

## Considered Options

- Extend only `ContextUsage` and the `events.context_usage` payload. Rejected: `ContextUsage` holds only the latest response, and the events payload cannot keep each attempt's estimate next to its reported counts.
- Add cache counts to the charged total. Rejected: OpenRouter's `prompt_tokens` already includes them, so cached tokens would be charged twice.
- Send cache and cost details to Home over Federation. Rejected: Home settles only the complete total, and a protocol change would add compatibility work with no consumer.

## Consequences

- Local settlement stops reporting partial totals, matching remote `complete_reported_usage`.
- The executing node owns the record. Federation and `generation_remote_usage` are unchanged.
- Metrics emit cache samples only when the provider reports them.
- Usage Records are visible only within the owning Tenant and contain no prompt text.
