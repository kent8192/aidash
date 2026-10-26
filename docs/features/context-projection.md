# Context Projection Versions

A Projection Version is the rule that turns a Run's context into the bytes of a
model request. An Agent definition version names it, the Run's Binding snapshot
pins it, and it never changes while the Run exists. Runs created before this
feature, and Agent definitions that name no version, use `legacy`.

| Version   | Request shape                                                                                                                           |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `legacy`  | One JSON user message with alphabetically ordered keys. This is the default.                                                            |
| `ordered` | A Tenant Cache Salt line at the start of `system`, then one user message with two text parts: a Stable Prefix part and a volatile part. |
| `native`  | A Tenant Cache Salt line at the start of `system`, enveloped attachments, then model-native assistant and tool messages, then the step. |

## Selecting `ordered`

Declare the versions a model accepts, then name one in the Agent definition:

```json
{
  "provider": "openrouter",
  "model_id": "vendor/model",
  "endpoint": "https://openrouter.ai/api/v1",
  "provider_credential": "openrouter",
  "context_window": 128000,
  "max_output_tokens": 8192,
  "modalities": ["text"],
  "cost": {},
  "projection_versions": ["legacy", "ordered"]
}
```

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Do the task.",
  "projection_version": "ordered"
}
```

Agent registration, publication and installation reject a version that the
pinned model does not declare. A model that omits `projection_versions` accepts
only `legacy`. Both fields are omitted from stored definitions when absent, so
existing definitions, digests and Binding snapshots keep their bytes. Run
creation checks the version again and rejects it if this node cannot render it.
It never falls back to `legacy`. Migration `registry/0017_projection_versions`
lets the registry database constraints accept both keys.

## Selecting `native`

Declare `native` in the model's `projection_versions` and name it in the Agent
definition, exactly as for `ordered`. `native` is salted too, so its Runs need a
Cache Salt Key and Tenant-scoped execution. Registration, Run creation and the
OpenRouter adapter each reject `native` on a model that does not declare it;
the adapter fails with a typed `UnsupportedProjection` error before any provider
traffic, and nothing ever falls back to `legacy`.

A `native` request has this shape:

1. `system`: the salt line, the Harness Contract and every harness instruction,
   including the per-turn catch-up and media-intake paragraphs. Under the
   `structured@*` [Harness Contracts](harness-contract.md), the per-turn
   paragraphs and the step's markers move to an `aidash`-trust envelope at the
   end of the trailing user message instead.
2. One user message of Run-stable attachments: `identity`, `task`,
   `reference_documents`, `run_message_summary` and the Execution Summary, each
   a separate text part.
3. The history. Each recorded Model Turn becomes an `assistant` message with its
   exact `tool_calls`, followed by one `tool` message per call with the same
   `tool_call_id`. A Human answer becomes a user message. Media observations
   become user messages.
4. One trailing user message with this step's volatile content (agent state,
   workspace observation, run messages, `semantic_memory`, deferred reads and
   the continuation markers no rendered Model Turn has answered yet) and this
   step's media parts. It is never stored in history. A citation correction
   asks for a corrected answer, so it stays through tool-call turns that
   gather evidence until a turn without calls answers it.

Everything derived from data is wrapped in a provenance envelope,
`{"source", "provenance", "revision", "trust": "data", "content"}`: attachments,
tool results, media observations, the question a Human answer replies to, the
step's volatile content and the text labels of its media parts. Only the Human
answer itself is unwrapped. Data never enters `system`. Request budgeting
reserves the enveloped size of media labels before it bounds the snapshot.

Only `native` Runs record Model Turns: once per accepted response, the Context
Journal gains an `assistant` event with the response text and the IDs of its
tool calls, before the first result. A final answer is a Model Turn without
calls. Reasoning output is never recorded. A call that never ran, because an
earlier call of the same response waited for a human or deferred its result, is
left out of the rendered assistant message, as `legacy` leaves it out of
history. A response without text whose calls never ran renders nothing and is
no Model Turn. Text beside calls that the citation gate withholds is neither
published nor recorded; its turn keeps only the calls. A request never carries
part of a Model Turn; validation rejects one before sending.

Context recovery works by Model Turn. Jev protects the first and the latest
`preserve_recent` Model Turns. A result Jev drops is replaced by
`{"elided": true, "invocation": "<call id>"}` under the same call, and a turn
without text whose every call is dropped is removed whole. The Summary Stage
absorbs whole turns that reached an accepted inference and lie outside the
protected turns.

With `prompt_cache: explicit`, the second breakpoint sits on the last history
message, or on the last attachment when there is no history.

## Cache Salt Key

`ordered` requests start `system` with
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
keys creates only `legacy` Runs: creating an `ordered` Run fails, and startup
logs a warning. To rotate, add a new key version and change `current`. Later
requests use the new version, and the only effect is cache misses. The OpenRouter
adapter adds the salt just before sending. Request metadata, compaction input,
logs and diagnostics show only the key version. A remote Run is salted with the
receiving node's local Tenant and that node's key. Because the salt needs a
Tenant, `ordered` Runs are created only through Tenant-scoped execution. Legacy
claims and legacy remote admission run in Workspaces without a Tenant, so they
reject `ordered` Agents even on a node that has keys.

## What stays stable

The Stable Prefix part holds `identity`, `task`, `reference_documents`,
`run_message_summary`, the adopted Execution Summary as `summary` (only when a
Context Policy's Summary Stage produced one), and `history`, in that order. The
volatile part follows:
it holds the agent state, workspace observation, run messages, deferred markers,
`semantic_memory` and per-turn instructions such as run-message catch-up and
media intake. Two consecutive steps share their bytes through all earlier
history.

These events legitimately end the shared prefix:

- History compaction, including a Summary Stage merge, or removal of an old
  media observation.
- A change to the task, the instructions, or a loaded Skill body.
- Run-message catch-up and media-intake turns, which narrow or clear the tools.
- Fixed content (instructions, tools, reference documents, per-turn
  instructions) so large that the Run-stable snapshot quota no longer fits
  beside it: the snapshot is then truncated to the remaining headroom, as in
  `legacy`, instead of failing the request.
- A different tool set.

For `ordered` and `native` Runs, a semantic retrieval is reused across steps while its
Retrieval Key is unchanged. The key covers the query inputs, a Run-fixed budget,
the binding, the authorization scope, and the policy, index and memory-binding
revisions. It also covers a digest of the Workspace's semantic entries and of
the revisions of the memory banks the Run recalls. An inserted, edited, deleted
or reindexed entry, or a changed memory unit, therefore triggers a fresh
retrieval. A remote Run can't read the Home's database, so the Home returns the
digest in the reply to the same describe request that refreshes the Run's
authority (peer protocol `0.3`). A Home on protocol `0.2` sends no digest, and
the key then has no corpus component: its remote Runs keep reusing a retrieval
until another component changes. The budget never exceeds half of the request's
remaining headroom, the same share `legacy` uses, so oversized fixed content can
change the budget and therefore the key.
Every reuse is rechecked against current authority first: revoked or narrowed
access pauses the Run, and a stale result is retrieved again once.

A remote `ordered` Run reuses a retrieval the Home performed for an earlier
step, so the Home has no semantic operation for the current step. Its inference
and compaction admissions therefore cite the operation whose receipt the step
reuses. The Home admits a cited operation only when it is READY for the same
Run and grant at or before the current step, the grant is live, no source the
Run consumed changed, and the Workspace corpus digest still equals the one
recorded before that retrieval. A revoked grant pauses the Run, and an edited
consumed source invalidates it as before (a follow-up Task continues the work).
Any other corpus change, such as a new entry, answers `stale`: the Run drops the
cached retrieval and retrieves once more. See [protocol](../protocol.md).

## Explicit prompt caching

Some routes cache a prompt prefix on their own. Others cache only up to a
`cache_control` breakpoint in the request. A model version declares which
applies with `cache_mode`:

| `cache_mode`      | Meaning                                                                                      |
| ----------------- | -------------------------------------------------------------------------------------------- |
| `none` or omitted | No prompt caching is assumed.                                                                |
| `automatic`       | The route caches matching prefixes without request markers.                                  |
| `explicit`        | The route caches only up to `cache_control` breakpoints. Accepted only for `anthropic/` IDs. |

An Agent version on such a model opts in with `prompt_cache` (`off`, the
default, or `explicit`):

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Do the task.",
  "projection_version": "ordered",
  "prompt_cache": "explicit"
}
```

`prompt_cache: explicit` requires `projection_version: ordered` or `native` and a model
that declares `cache_mode: explicit` for an allowlisted ID. Registration,
publication, installation and each inference check this pairing. An opt-in is
never dropped silently, and no other route ever receives `cache_control`.

An opted-in request carries two `{"type":"ephemeral"}` breakpoints: one on
`system`, sent as a single text block that starts with the Tenant Cache Salt,
and one on the Stable Prefix part. The volatile part and media parts are never
marked. Run-message catch-up and media-deferral steps narrow or clear the
tools, which precede the cached prefix, so they carry no breakpoints. Request
estimates always count the breakpoint framing for `ordered`, so opting in never
changes a fitting or compaction decision. Both fields are omitted from stored
definitions when absent. Migration `registry/0018_prompt_cache` lets the
registry database constraints accept them.

Cache writes cost more than ordinary input (1.25× on Anthropic routes), so
enable explicit caching for Agents whose Runs take several steps. The matched
comparison on `anthropic/claude-haiku-5.5` is recorded in
[the explicit-caching evidence](../operations/evidence/2026-10-10-prompt-cache-explicit/README.md).

## Operations

Register `ordered` Agent versions only after every worker and receiving peer
that may run them is upgraded and configured with a Cache Salt Key; see
[Worker activation](../operations/worker-activation.md). Cache-hit metrics come
from the usage ledger (#177).
