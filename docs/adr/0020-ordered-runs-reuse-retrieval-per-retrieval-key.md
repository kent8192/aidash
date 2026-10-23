# Ordered Runs reuse a semantic retrieval while its Retrieval Key holds, and re-retrieve once on a stale reuse

Decided on 2026-10-10 (#172). Today the Source observation that caches `semantic_memory` and `skill_context` is keyed by `step:input_seq:task.revision`, so it only covers recovery of the same inference boundary and retrieval runs on every step. On the Ordered Projection Version, a Run instead reuses its retrieval while its Retrieval Key is unchanged: the query inputs (task title and description, and the digest of each inline run message), the Run-fixed retrieval budget of `(window − max_output) / 8` bytes, the binding digest, the authorization scope (Tenant and subject), the Tenant policy revision for local Runs, the semantic index revision and the memory participant revision. `skill_context` is cached apart, keyed by the Run's Skill record revision. Legacy Runs keep today's per-step behavior and bytes.

Every reuse is rechecked against current authority first. A `Forbidden` recheck pauses the Run as before. A `Conflict` means a revision changed that the key did not capture; the cache is discarded and the step retrieves once more through the normal, fully authorized path. The memory envelope's `boundary` holds the Retrieval Key components instead of `step` and `run_revision`, and the memory recall operation is derived from it, so retrieving again under the same key replays identical bytes from the operation ledger.

## Considered Options

- Keep `step` in the key. Rejected: the cache would never be reused within a Run.
- Keep the per-step budget `remaining / 2`. Rejected: it shrinks as history grows, so the key would change on almost every step. `semantic_memory` sits after the Stable Prefix, so its bytes do not affect provider caching; the gain is fewer searches, embeddings and memory read journals.
- Fail the Run on a `Conflict` during reuse, as recovery does today. Rejected: a cache that lives for a whole Run meets more revision changes than a recovery replay, and failing would turn routine index updates into failed Runs.
- Add Home's Tenant policy revision to remote admission so remote keys include it. Rejected: the per-step `describe` recheck on Home already decides authority before any reuse.

## Consequences

- No cached retrieval is ever served without a passing recheck, so revocation and narrowing still stop the Run at the next boundary (ADR 0013).
- The cache lives in the Run's Context, so it is never shared across Runs or Tenants.
- A fixed budget can make compaction start earlier on long Runs; near-limit tests guard that compaction stays no more frequent than with Legacy.
- Memory reads are journaled when retrieved, not on every reuse; the Run-wide journal recheck still covers every reused unit.

## Indexing while a Run is active

Decided on 2026-10-23 (#342): a new retrieval whose authorized candidates include a pending entry, an old index revision or changed source bytes waits for the complete index. Local and required-Home Runs use the existing bounded semantic retry policy and pause with `retries_exhausted` if indexing does not finish. No partial READY subset is returned, and an indexing wait never becomes Degraded context. An Ordered required-Home Run may first receive `stale` for its old observation; that recovery discards the cache before the fresh read waits for indexing. An explicit search outside a Run retains its incomplete-index conflict so a person can inspect and reindex it.
