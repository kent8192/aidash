# Context Projection Versions

A Projection Version is the rule that turns a Run's context into the bytes of a
model request. An Agent definition version names it, the Run's Binding snapshot
pins it, and it never changes while the Run exists. Runs created before this
feature, and Agent definitions that name no version, use `legacy`.

| Version   | Request shape                                                                                                                           |
| --------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `legacy`  | One JSON user message with alphabetically ordered keys. This is the default.                                                            |
| `ordered` | A Tenant Cache Salt line at the start of `system`, then one user message with two text parts: a Stable Prefix part and a volatile part. |
| `native`  | Reserved for model-native messages (#178). Registration rejects it.                                                                     |

## Selecting `ordered`

Declare the versions a model accepts, then name one in the Agent definition:

```json
{
  "provider": "openrouter",
  "model_id": "vendor/model",
  "endpoint": "https://openrouter.ai/api/v1",
  "credential_env": "AIDASH_SECRET_OPENROUTER",
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
`run_message_summary` and `history`, in that order. The volatile part follows:
it holds the agent state, workspace observation, run messages, deferred markers,
`semantic_memory` and per-turn instructions such as run-message catch-up and
media intake. Two consecutive steps share their bytes through all earlier
history.

These events legitimately end the shared prefix:

- History compaction, or removal of an old media observation.
- A change to the task, the instructions, or a loaded Skill body.
- Run-message catch-up and media-intake turns, which narrow or clear the tools.
- Fixed content (instructions, tools, reference documents, per-turn
  instructions) so large that the Run-stable snapshot quota no longer fits
  beside it: the snapshot is then truncated to the remaining headroom, as in
  `legacy`, instead of failing the request.
- A different tool set.

For `ordered` Runs, a semantic retrieval is reused across steps while its
Retrieval Key is unchanged. The key covers the query inputs, a Run-fixed budget,
the binding, the authorization scope, and the policy, index and memory-binding
revisions. For local Runs it also covers a digest of the Workspace's semantic
entries and of the revisions of the memory banks the Run recalls. An inserted,
edited, deleted or reindexed entry, or a changed memory unit, therefore triggers
a fresh retrieval. Remote Runs can't observe the Home's content yet (#191). The budget
never exceeds half of the request's remaining headroom, the same share `legacy`
uses, so oversized fixed content can change the budget and therefore the key.
Every reuse is rechecked against current authority first: revoked or narrowed
access pauses the Run, and a stale result is retrieved again once.

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

`prompt_cache: explicit` requires `projection_version: ordered` and a model
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
