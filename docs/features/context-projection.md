# Context Projection Versions

Native is the Projection Version for every Run step. It renders the Context
Projection as model-native messages and uses the Tenant Cache Salt and an
opaque sticky-routing identifier. An Agent definition may name
`projection_version: "native"`. Omitting the field selects Native, including
when a stored definition or Binding snapshot is read. Models use Native
automatically.

Registration rejects the retired Projection Versions. A Run whose saved
snapshot explicitly pins a retired version fails at its next worker step
with `PROJECTION_RETIRED`. Existing definitions, snapshots, digests and Usage
Records keep their stored values. See [ADR 0144](../adr/0144-native-is-the-only-projection-version-and-model-requests-outside-run-steps-are-model-calls.md).

## Model requests

A Run step sends:

1. The `system` message with its Tenant Cache Salt, the
   [Harness Contract](harness-contract.md), and pinned instruction text.
2. One user message of Run-stable attachments: `identity`, `task`,
   `reference_documents`, `run_message_summary` and the Execution Summary, each
   a separate text part.
3. The history. Each recorded Model Turn becomes an `assistant` message with its
   exact `tool_calls`, followed by one `tool` message per call with the same
   `tool_call_id`. A Human answer becomes a user message. Media observations
   become user messages.
4. When present, a trailing user message carrying this step’s volatile
   evidence and media. Changed bookkeeping, Harness requirements, and
   admitted run messages and thread observations are journaled and appear
   in history at the position where they were recorded.

Compaction, memory, Workbench, media checks and MCP sampling use Native
Model Calls. The Model Call owns its Inference Attempts and records the
digest of the request and its usage.

Everything derived from data is wrapped in a provenance envelope,
`{"source", "provenance", "revision", "trust": "data", "content"}`: attachments,
tool results, media observations, the question a Human answer replies to, the
step's volatile content and the text labels of its media parts. Only the Human
answer itself is unwrapped. Data never enters `system`. Request budgeting
reserves the enveloped size of media labels before it bounds the snapshot.

Once per accepted response, the Context
Journal gains an `assistant` event with the response text and the IDs of its
tool calls, before the first result. A final answer is a Model Turn without
calls. The provider's reasoning is recorded on the same event exactly as
received, unless the model sets `reasoning_passback: false`, and every later
request resends it unchanged: Chat `reasoning_details` (or plain reasoning text)
on the assistant message, Responses reasoning items before the turn's output
(#942). A turn recorded at or after a revoked or expired result does not resend
its reasoning; routes that require the field get an empty reasoning string.
Compaction steps drop the reasoning of turns older than the protected recent
tail, and the Journal export leaves reasoning out. A call that never ran, because an
earlier call of the same response waited for a human or deferred its result, is
left out of the rendered assistant message. A response without text whose calls never ran renders nothing and is
no Model Turn. Text beside calls that the citation gate withholds is neither
published nor recorded; its turn keeps only the calls. A request never carries
part of a Model Turn; validation rejects one before sending.

Context recovery works by Model Turn. Jev protects the first and the latest
`preserve_recent` Model Turns. A result Jev drops is replaced by
`{"elided": true, "invocation": "<call id>"}` under the same call, and a turn
without text whose every call is dropped is removed whole. The Summary Stage
absorbs whole turns that reached an accepted inference and lie outside the
protected turns.

With `prompt_cache: explicit`, the history breakpoint sits on the last history
message, or on the last attachment when there is no history.

## Cache Salt Key

Run step requests start `system` with
`aidash-cache-scope:v{version}:{hex}`. Here `hex` is an HMAC-SHA256 of the Run's
Tenant ID under the node's Cache Salt Key, truncated to 128 bits. Provider
prefix caches therefore never match across Tenants. Configure the key in the
node settings:

```toml
[cache_salt]
current = 1

[[cache_salt.keys]]
version = 1
secret = "${AIDASH_CACHE_SALT_KEY_V1}"
```

Use a long random secret, and never reuse a provider API key. A node without
configured keys derives version 1 from `core.secret_key`, so new Native Agents run
without any setup. Configure keys to rotate the salt on its own. To rotate, add a new key version and change `current`. Later
requests use the new version, and the only effect is cache misses. The OpenRouter
adapter adds the salt just before sending. Request metadata, compaction input,
logs and diagnostics show only the key version. A remote Run is salted with the
receiving Node's local Tenant and that Node's key. Every Run requires a Tenant
and the executing Node's Cache Salt Key.

## What stays stable

The Stable Prefix holds the system message, Run-stable attachments and
the history. Consecutive steps reuse the prefix through earlier unchanged
messages.

These events end the shared prefix:

- History compaction, including a Summary Stage merge, or removal of an old
  media observation.
- A change to the task, the instructions, or a loaded Skill body.
- Run-message catch-up and media-intake turns, which narrow or clear the tools.
- Fixed content (instructions, tools, reference documents, per-turn
  instructions) so large that the Run-stable snapshot quota no longer fits
  beside it. The snapshot is truncated to the remaining headroom.
- A different tool set.

A Run reuses a semantic retrieval across steps while its
Retrieval Key is unchanged. The key covers the query inputs, a Run-fixed budget,
the binding, the authorization scope, and the policy, index and memory-binding
revisions. It also covers a digest of the Thread's semantic entries and of
the revisions of the memory banks the Run recalls. An inserted, edited, deleted
or reindexed entry, or a changed memory unit, therefore triggers a fresh
retrieval. A remote Run can't read the Home's database, so the Home returns the
digest in the reply to the same describe request that refreshes the Run's
authority (peer protocol `0.3`). A Home on protocol `0.2` sends no digest, and
the key then has no corpus component: its remote Runs keep reusing a retrieval
until another component changes. The budget never exceeds half of the request's
remaining headroom, so oversized fixed content can
change the budget and therefore the key.
Every reuse is rechecked against current authority first: revoked or narrowed
access pauses the Run, and a stale result is retrieved again once.

A remote Run reuses a retrieval the Home performed for an earlier
step, so the Home has no semantic operation for the current step. Its inference
and compaction admissions therefore cite the operation whose receipt the step
reuses. The Home admits a cited operation only when it is READY for the same
Run and grant at or before the current step, the grant is live, no source the
Run consumed changed, and the Thread corpus digest still equals the one
recorded before that retrieval. A revoked grant pauses the Run, and an edited
consumed source invalidates it as before (a follow-up Task continues the work).
Any other corpus change, such as a new entry, answers `stale`: the Run drops the
cached retrieval and retrieves once more. See [protocol](../protocol.md).

## Prompt caching

Prompt caching is on by default for new definitions ([ADR 0115](../adr/0115-prompt-caching-is-on-by-default-and-its-method-follows-the-route.md)). Registration writes the settings below into a definition that names none. Stored definitions and pinned Runs do not change.

A model version declares its route's behaviour with `cache_mode`:

| `cache_mode`      | Meaning                                                                                           |
| ----------------- | ------------------------------------------------------------------------------------------------- |
| `none` or omitted | No prompt caching is assumed.                                                                     |
| `automatic`       | The route caches matching prefixes without request markers.                                       |
| `explicit`        | The route caches up to `cache_control` breakpoints. Accepted only for the IDs in the table below. |

Registration writes `explicit` on a new model whose ID is in the table, on OpenRouter's Chat API at `https://openrouter.ai/`. An Agent version uses it through `prompt_cache` (`off` or `explicit`). Registration writes `explicit` on a new Agent when its model accepts breakpoints. Both opt-outs are explicit: `prompt_cache: off`, or `cache_mode: none` on the model.

| Model IDs                                  | What a request sends                                                                          |
| ------------------------------------------ | --------------------------------------------------------------------------------------------- |
| `anthropic/`                               | Breakpoints on `system` and the Stable Prefix.                                                |
| `openai/gpt-5.6`, `openai/gpt-6`           | Those, one on the previous step's position, and `prompt_cache_options: {"mode": "explicit"}`. |
| Anything else, such as Gemini and DeepSeek | No markers. These routes cache on their own, Gemini unevenly.                                 |

A breakpoint is `{"type":"ephemeral"}`. `system` becomes a single text block that starts with the Tenant Cache Salt. The volatile part and media parts are never marked. Run-message catch-up and media-deferral steps narrow or clear the tools, which precede the cached prefix, so they carry no breakpoints. Request estimates always count the breakpoint framing, so turning caching on never changes a fitting or compaction decision.

`prompt_cache: explicit` requires a model that declares `cache_mode: explicit` for a listed ID. Registration, publication, installation and each inference check this pairing. An opt-in is never dropped silently, and no other route ever receives `cache_control`.

Without markers, GPT-5.6 and later write through the last message of every request at 1.25× the input price and read only the start of the prefix. `cache_mode: none` sends explicit mode there too, which really turns caching off. A model that omits `cache_mode` keeps its implicit writes, so its stored bytes do not change.

### Staying on one provider

Caches are local to a provider, so every Run step sends `session_id`, and OpenRouter keeps the Run on one provider while it can. The value is `aidash-session-` followed by an HMAC of the Tenant and the Run under the Cache Salt Key, truncated to 128 bits. It reaches OpenRouter and its Broadcast destinations, so it names neither. The broker rejects any other form.

### Costs to expect

Cache writes cost 1.25× ordinary input on Claude and GPT-5.6 or later. Reads cost 0.1× on most models and 0.05× on Claude Opus 5.5, Claude Sonnet 5.5 and GPT-6.1 Sol and Terra. Writing once and reading once already costs less than two uncached requests, so only an Agent that makes one model call per Run should turn caching off.

Removing tokens from the middle of history makes the provider rewrite everything after them. Claude Haiku 5.5 charges five times the price above 100,000 prompt tokens, counting cached tokens.

The evidence is in [the route check](../operations/evidence/2026-11-07-prompt-cache-routes/README.md) and [the first explicit-caching comparison](../operations/evidence/2026-10-10-prompt-cache-explicit/README.md).

## Operations

Configure Cache Salt Keys on every executing Node. See
[Worker activation](../operations/worker-activation.md). Cache-hit metrics come
from the usage ledger (#177) and `aidash_model_tokens_total`; see
[HTTP observability](../operations/http-observability.md).
