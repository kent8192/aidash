---
status: accepted
---

# Prompt caching is on by default for new definitions, and its method follows the route

Decided on 2026-11-07 (#776). This replaces ADR 0019's opt-in. ADR 0019 still describes the marker positions, the pinning per Run and the Cache Mode.

Caching was off in every Run we measured, and one route lost money. Claude read 0 of 10.9M input tokens. GPT-6.1 Terra paid 13.5 % more than it would have with no cache at all, because it wrote 3.03M tokens at 1.25× and never read them. ADR 0019 kept caching opt-in because short Runs pay more, but a 5-minute cache breaks even at two requests and the shortest measured Run made four calls.

A new model definition on an allowlisted OpenRouter Chat route now gets `cache_mode: explicit` when it names no Cache Mode. A new Agent definition gets `prompt_cache: explicit` when it is on Native with a structured Harness Contract and its model accepts breakpoints. As in ADR 0093, the values are written into the definition, and stored definitions and Binding snapshots do not change. `prompt_cache: off` and `cache_mode: none` opt out.

The method is chosen per route. The [live check](../operations/evidence/2026-11-07-prompt-cache-routes/README.md) confirmed each row, first against the routes directly and then with real Runs. In real Runs, Claude and GPT read their whole history on every step and cost 48 % and 46 % less than the same Agent with caching off:

| Route                                                     | Request                                                                                      |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Claude                                                    | `cache_control` at the end of `system` and of history                                        |
| GPT-5.6 and later                                         | The same, plus the previous step's position, plus `prompt_cache_options: {mode: "explicit"}` |
| Gemini, DeepSeek and other routes that cache on their own | No markers                                                                                   |

Every step of a salted Run also sends `session_id`, so the Run stays on one provider. The value is an HMAC of the Tenant and the Run under the Cache Salt Key, because it reaches OpenRouter and its Broadcast destinations. The broker admits only that form, and only `{"mode": "explicit"}` as `prompt_cache_options`.

The method depends on the model, not on the Tenant's Data Protection Level. The check measured the same behaviour with and without ZDR on Claude and GPT, and a modest difference on Gemini.

## Considered Options

- **Keep the opt-in and document it.** Rejected: three settings had to line up and the dashboard offered none of them, so nobody had it on. A default that loses money on a route is also not neutral.
- **Enable it for stored definitions too.** Rejected: registered definitions are immutable, and a pinned Run keeps its bytes.
- **Top-level automatic `cache_control`.** Rejected: it places the breakpoint on the last block, which here is the volatile step message, so every step would write bytes it never reads. The broker rejects it as well.
- **GPT with implicit mode and two markers.** It works, and it needs no new parameter. Rejected as the default: its write also covers the volatile message at 1.25×, and a real step's volatile part may be large. Measured writes per step were 583 against 378.
- **GPT with explicit mode and two moving markers.** Rejected: explicit mode looks up only the breakpoints in the request, so the read stopped at `system` in the check.
- **Markers on Gemini.** Rejected: OpenRouter uses only the last one, and a moving one read nothing. `session_id` alone reads with no write cost, though unevenly: a real Run read 41 % of its input, and 2 of 8 late steps read nothing.
- **A one-hour TTL.** Not taken now. Replaying measured Runs, it cost workers more and saved money only for an Agent that waits a long time. It stays a possible per-Agent option, and the broker would need to admit it.
- **Pinning a host on mixed-host routes.** Not needed: `session_id` held DeepSeek on one host in the check. `provider_route` remains available.
- **Dropping the slug allowlist for a declaration.** Not taken now, although name checks are the largest cause of silent caching faults in published work (arXiv 2610.34817). A wrong declaration would send markers to a route nobody checked, so each prefix on the list waits for a live check.

## Consequences

- The volatile message is resent at full price on every step and sets the ceiling on what a Run can read. It was 2,000 to 3,500 tokens in the real Runs, which read 61 % to 71 % of their input at 9,000 to 15,000 tokens per request. Its share should fall as history grows, which those short Runs did not test.
- Gemini reads unevenly, so its savings cannot be counted on the way Claude's and GPT's can.
- A node with no `[cache_salt]` keys now derives version 1 from `core.secret_key`. No shipped Compose file or Helm chart set a key, so without this a new Native Agent's Run was rejected, and default-on caching could not work out of the box. The derivation is one-way, and changing the secret only causes cache misses. This amends ADR 0016, under which such a node created only Legacy Runs.
- `session_id` is sent on Ordered and Native requests whenever the Run is known, including Runs of definitions stored before this decision. It changes where a request is routed, not what the model sees or any digest. Legacy requests never carry it, since they have no Cache Scope.
- A step without markers on a GPT route, such as a catch-up turn, uses no cache and pays no write. On those routes only explicit mode with no markers really turns caching off, so `cache_mode: none` sends it as well. An omitted Cache Mode keeps a stored definition's bytes, including its implicit writes.
- Compaction costs more with a cache. Removing X tokens from the middle of history makes the provider rewrite the S tokens after it, and later steps save only the read price. That pays back after about 11.5 × S / X steps on Claude Haiku, and 24 × S / X where reads cost 0.05×.
- Claude Haiku 5.5 charges five times the price above 100,000 prompt tokens, counting cached tokens. Caching does not avoid that tier. Keep `context_window` below it where the cost matters.
- `aidash_model_tokens_total` gains a bounded `family` label so a loss on one provider is visible. A cache that fails stays silent otherwise: requests succeed and the bill rises.
