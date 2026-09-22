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
Existing stored model configurations therefore remain readable without a data
migration or re-registration. Previously, inference inherited the shared HTTP
client's 120-second total timeout, which could interrupt valid long-running
completions and trigger retries.

Explicit values belong to the model's versioned configuration. To change an
explicit timeout, register a new model version and update the agents that should
use it; do not mutate an immutable published version. Rust callers constructing
`ModelConfig` directly must include `request_timeout_secs: None` (the default) or
`Some(seconds)` in their struct literals.

## Scope

Only inference requests override the shared client's total timeout. Other
outbound traffic retains the shared 120-second default or its own existing
request-specific limit, such as the model catalog's 15 seconds. The existing
10-second connection timeout, redirect policy, credentials, response-size bound,
and retry behavior are unchanged.

This setting does not extend a provider, gateway, or reverse proxy's independent
deadline. A request that reaches Aidash's configured deadline can still fail and
follow the existing retry policy; this change does not guarantee exactly-once
billing.

## Regression tests

Run `cargo test --locked --test provider_timeouts --test providers`. The timeout
tests use a local HTTP fixture and Tokio's virtual clock, not paid model calls or
multi-minute wall-clock sleeps. They cover a 508-second completion beyond the old
120-second cutoff, a configured limit beyond the 900-second default, shorter
configured limits, the default deadline, unchanged non-inference deadlines, and
configuration validation and legacy deserialization.
