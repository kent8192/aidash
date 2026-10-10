# Inference request timeouts

A registered model can set `config.request_timeout_secs` to a positive integer
number of seconds. This is Aidash's total HTTP deadline for each inference
request, from connection establishment through reading the complete response
body. It is a local transport setting, not a parameter sent to OpenRouter.

For example, include the following configuration when registering a model:

```json
{
  "provider": "openrouter",
  "model_id": "vendor/model",
  "endpoint": "https://openrouter.ai/api/v1",
  "credential_env": "AIDASH_SECRET_OPENROUTER",
  "request_timeout_secs": 900,
  "context_window": 32768,
  "max_output_tokens": 4096,
  "modalities": ["text"],
  "cost": {}
}
```

Use the actual model ID, context window, and output limit from the model catalog.
Choose the timeout for the selected model's expected latency and your workload.
Values above 900 seconds, such as 1200, are honored rather than capped at the
default. Zero, negative, fractional, string, and out-of-range values are rejected;
the supported integer range is `1..=4294967295` seconds.

## Defaults and compatibility

Omitting `request_timeout_secs`, or setting it to `null`, uses **900 seconds**.
Existing stored model configurations remain readable without rewriting their
data or re-registering them. Previously, inference inherited the shared HTTP
client's 120-second total timeout, which could interrupt valid long-running
completions and trigger retries.

The normal database upgrade applies
`m20260922_134000_inference_timeout_constraints`. This migration updates and
revalidates the model registration and installation-override validators to accept
the new field. Historical migrations and existing configuration values are not
changed. Downgrading refuses configurations containing the new field rather than
silently discarding their settings.

Explicit values belong to the model's versioned configuration. To change an
explicit timeout, register a new model version and update the agents that should
use it; do not mutate an immutable published version. Rust callers constructing
`ModelConfig` directly must include `request_timeout_secs: None` (the default) or
`Some(seconds)` in their struct literals.

## Scope

Only inference requests override the shared client's total timeout. Other
outbound traffic retains the shared 120-second default or its own existing
request-specific limit, such as the model catalog's 15 seconds. The existing
10-second connection timeout, redirect policy, credentials, and response-size
bound are unchanged.

Cancellation does not wait for the inference deadline. During inference, the
worker checks committed run control with a 250-millisecond pause between reads,
independently of lease renewal and process-local notifications. Cancellation drops
the pending request, including a stalled response-body read, and follows the
existing durable cancellation path without retrying inference. It does not
interrupt unrelated tool execution or durable transitions.

This setting does not extend a provider, gateway, or reverse proxy's independent
deadline. A request that reaches Aidash's configured deadline can still fail and
follow the existing retry policy. Closing a local HTTP request does not guarantee
that an upstream provider stops processing or waives charges; this change does
not guarantee exactly-once billing.

## Streamed responses

Inference streams by default. A model that omits `config.streaming`, or sets
it to `null` or `true`, requests a server-sent event stream from OpenRouter
(`"stream": true` with `stream_options.include_usage`). Setting
`streaming: false` opts out and keeps the non-streamed request. Some
OpenAI-compatible endpoints ignore `stream`: a 2xx answer with Content-Type
`application/json` is read with the same 1 MiB cap and validated as a
non-streamed completion, without progress. Any other content type than
`text/event-stream` or `application/json` fails the attempt. Aidash assembles
the complete stream, including the final
`finish_reason`, usage, and `[DONE]` marker, and validates it exactly like a
non-streamed completion before any tool call can run. A stream that ends early,
is truncated, refused, oversized (more than 1 MiB assembled), or reports a
provider error fails with the same errors as the equivalent non-streamed
response. Every received byte, including reasoning, unknown fields and
keepalives, also counts toward a 128 MiB cap on the whole stream, which leaves
room for the per-chunk envelope of one-token deltas. Like the non-streamed
`/choices/0`, only choice `0` is assembled; a chunk with several choices that
omit `index`, or with choice `0` more than once, is rejected as ambiguous.
After choice `0` reports its `finish_reason`, any further text, refusal, tool
call or finish reason for it fails the stream; usage-only chunks are still
accepted. SSE lines may end in LF, CRLF or a bare CR. While the stream is open,
only display
progress is published: text, and each tool call's ID, name and argument size.
Argument text and provider reasoning are never published.

`config.stream_stall_timeout_secs` is the longest silence, in seconds, between
streamed data events, including the wait for response headers. Provider
keepalive comments such as `: OPENROUTER PROCESSING` do not reset it; reasoning
chunks do, even though their content is dropped. Omitting it, or setting it to
`null`, uses **120 seconds**; the supported range is `1..=4294967295`. A stall
fails the inference attempt and follows the same retry path as a transport
timeout. `request_timeout_secs` still bounds the complete streamed request.

Both settings can be installation overrides. The registry migration
`registry/0019_model_streaming_config` (after `registry/0018_prompt_cache`)
adds them to the current `registry_model_config` allowlist read from the catalog,
keeping earlier keys such as `provider_credential`, `projection_versions` and
`cache_mode`. The separate `registry_model_streaming` check validates their
types, and the installation-override guard accepts them; omitted settings stay
unserialized, so existing configuration digests are unchanged.

## Regression tests

Run `cargo test --locked -p aidash-server --test providers` and
`cargo test --locked -p aidash-integrations inference`. The timeout
tests use a local HTTP fixture and Tokio's virtual clock, not paid model calls or
multi-minute wall-clock sleeps. They cover a 508-second completion beyond the old
120-second cutoff, a configured limit beyond the 900-second default, shorter
configured limits, the default deadline, unchanged non-inference deadlines, and
configuration validation and legacy deserialization.

The streamed-response tests use a scripted local event stream. They cover
progress before the accepted response, equality with the non-streamed result,
missing `[DONE]` or `finish_reason`, duplicate call IDs, truncation, refusal,
keepalive-only stalls, oversized streams, that argument text never reaches
progress, a whole JSON completion answering a streamed request, and rejection
of other content types.

Run `scripts/test-rust.sh --coverage` for the full suite with disposable services.
This includes the PostgreSQL tests in the [inference cancellation suite](../../server/src/apps/execution/tests/inference_cancellation.rs) and
[provider timeout suite](../../server/src/apps/registry/tests/provider_timeout_persistence.rs): cancellation before headers and during
body reads, API registration, persisted overrides, invalid values, migration
upgrades, and safe downgrade behavior.
