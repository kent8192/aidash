# Explicit prompt caching is an Agent definition opt-in, pinned per Run and bounded by the model's Cache Mode

Decided on 2026-10-09 (#142). A model definition declares its Cache Mode: `none`, `automatic` or `explicit`. An Agent definition opts in to explicit caching with `prompt_cache: explicit`, which defaults to `off`. Like the Projection Version (ADR 0015), the choice is copied into the Run's Binding snapshot and is fixed for the Run. Agent registration accepts `explicit` only when all of these hold:

- the model declares `explicit`;
- the model's slug is on the adapter's explicit-cache allowlist, which today contains only `anthropic/`;
- the Projection Version is `Ordered` or later.

Only then does the adapter send `cache_control`, at two points: the end of `system`, and the last history part of the `Ordered` user message (the Run context part when the history is empty). The volatile current-state part is never marked. The TTL is the provider default of five minutes. Every other request carries no `cache_control` field.

## Considered Options

- Enable explicit caching for every Agent whose model declares `explicit`. Rejected: cache writes cost 1.25 times the input price, so Runs with few steps pay more, and the Agent definition is where that trade-off is known.
- A node-wide switch. Rejected for the same reason as in ADR 0015: new behavior is versioned and deliberately enabled per Agent definition, not per deployment.
- Trust the model declaration without a slug allowlist. Rejected: a misdeclared model would send `cache_control` to a route that does not support it.
- One-hour TTL. Rejected: it doubles the write cost, and a Run's steps normally fall within five minutes of each other.

## Consequences

- Gemini, OpenAI and DeepSeek routes declare `automatic` and rely on the stable Ordered layout alone.
- An omitted `prompt_cache` field is not serialized, so Agent definitions and Binding snapshots written before this field are unchanged.
- No breakpoint ends before `system` ends, so every cache entry includes the Tenant salt (ADR 0016).
- A step whose `system` or tools differ from the Run's ordinary turns, such as a message catch-up turn or a media deferral turn, carries no `cache_control`. Its prefix would not be reused, so a cache write would only add the 1.25× write surcharge. The ordinary prefix remains cached within the TTL.
- A provider reads an earlier entry only within 20 content blocks of the new breakpoint. A step that appends more than about 19 history events misses the history entry once; the `system` entry still hits.
