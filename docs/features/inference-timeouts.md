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

## Regression tests

Run `cargo test --locked --test provider_timeouts --test providers`. The timeout
tests use a local HTTP fixture and Tokio's virtual clock, not paid model calls or
multi-minute wall-clock sleeps. They cover a 508-second completion beyond the old
120-second cutoff, a configured limit beyond the 900-second default, shorter
configured limits, the default deadline, unchanged non-inference deadlines, and
configuration validation and legacy deserialization.

Run `scripts/test-rust.sh --coverage` for the full suite with disposable services.
This includes the PostgreSQL tests in `tests/inference_cancellation.rs` and
`tests/provider_timeout_persistence.rs`: cancellation before headers and during
body reads, API registration, persisted overrides, invalid values, migration
upgrades, and safe downgrade behavior.
