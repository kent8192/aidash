# Semantic memory

The [nonfunctional release requirements](operations/nonfunctional-release.md)
cover PostgreSQL/Qdrant backup consistency, restore drills, retention and alerts.
The recovery behavior below is not a tested RPO or RTO.

Aidash keeps authoritative sources, revisions, scope, indexing jobs, read dependencies,
and cleanup records in PostgreSQL. Qdrant stores vectors and identifiers. Retrieved text
always comes from the current authorized source, never from a vector payload.

## Configure and operate

`compose.yaml` includes Qdrant 1.19.1 with a persistent volume and a loopback-only port,
63370 by default. Start it with `docker compose up -d qdrant`. Set
`AIDASH_SECRET_TEST_QDRANT` in both Compose and the Aidash process to override the
explicit local fixture credential. Production deployments should use a private Qdrant
endpoint, TLS, separately managed credentials, and persistent storage with backups.
PostgreSQL remains necessary even when Qdrant is available.

Open **Semantic memory** in the dashboard, select a workspace, and configure its index
as an operator. The embedding adapter uses the OpenAI `POST /embeddings` contract.
The endpoint is the API base (for example `https://api.openai.com/v1`); model, model
version, and vector width are explicit. Use a provider's immutable model identifier
when available. `model_version` is operator-supplied provenance, not a claim that an
alias cannot change upstream. Responses must identify the selected model and contain
one finite, nonzero vector of the configured width. Credentials are `AIDASH_SECRET_*`
environment references; bearer values are never stored in index configuration.

The management API is under `/api/workspaces/{workspace}/semantic`:

| Operation       | Endpoint                     | Behavior                                                                                                                       |
| --------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Configure       | `POST /index`                | Operator only; optimistic `expected_revision` and complete `spec`. Every change creates a new immutable collection generation. |
| Inspect         | `GET /index`                 | Current configuration and revision.                                                                                            |
| Ingest/update   | `POST /entries`              | Stable `key`, `expected_revision`, `source`, optional qualified `agent`, and object `metadata`.                                |
| Inspect sources | `GET /entries`               | Authorized live sources, indexing state, attempt count, and sanitized failure.                                                 |
| Delete          | `DELETE /entries/{id}`       | Expected revision; immediate SQL tombstone and durable vector cleanup.                                                         |
| Reindex         | `POST /entries/{id}/reindex` | Expected revision; allocates a new point identity and retries with the requesting authority.                                   |
| Search          | `POST /search`               | Query, optional agent scope, metadata equality filters, result count, and token budget.                                        |
| History         | `GET /history`               | Latest authorized indexing transitions and retry decisions.                                                                    |
| Cleanup         | `GET /cleanup`               | Operator-only pending and failed point/collection deletion counts.                                                             |

Source forms are `{"kind":"memory","text":"..."}`, `{"kind":"message","id":"..."}`,
and `{"kind":"artifact","id":"..."}`. Linked content is loaded from the same workspace;
callers cannot substitute text or attach another workspace's source. A source's identity
is immutable. Memory text and metadata can be updated with a matching revision.
Identical creation/update retries reuse the existing result. Deleted keys are tombstoned
and cannot be reused; create a new explicit key for a distinct source.

A configured workspace also indexes its Agent's `memory_write` slot transactionally
with the existing memory update. The qualified Agent identity scopes that slot. Replayed
identical writes do not create extra source revisions. Deleting that slot is explicit;
deletion also clears the corresponding legacy memory slot (requiring `memory.write`).
Subsequent automatic writes report the tombstoned-key conflict instead of resurrecting it.
Managed slots are edited through `memory_write`; API source edits cannot change their scope.
Messages and artifacts are ingested explicitly through the API or dashboard.

## Authority, retrieval, and context

Operators manage provider endpoints. Scoped callers need `workspace.read` and the
workspace action `semantic.read`, `semantic.write`, `semantic.delete`, or
`semantic.search`. Individual semantic resources also require `semantic.read`,
`semantic.write`, or `semantic.delete`; their attributes include workspace owner,
creator, Agent scope, and metadata. Linked sources additionally require the underlying
message/artifact and task read permissions. Mutations returning a complete source also require
its `semantic.read` permission. Deny precedence, delegation intersection,
and live credential/policy leases use the same authorization engine as other resources.

Search first constructs a bounded allowlist from PostgreSQL. Qdrant receives only those
current point IDs and the workspace/tenant filters. Every returned ID, revision, payload
scope, permission, and current source is checked again before text is returned. An Agent
scoped source is included only when the requested Agent scope matches; ABAC can further
restrict who may request that scope. Unscoped sources are workspace-wide candidates.

The configured source limit is at most 1,024 live entries; inputs at most 32 KiB; results
at most 20. Metadata is at most 4 KiB. Result budgets include source provenance and JSON
overhead, and `truncated` explicitly reports excluded results. No candidate list is
silently cut off. A linked source that changed before reindexing blocks its authorized
search rather than returning a stale vector/text pairing.

With `auto_context`, local Agents retrieve against their current task at the inference
boundary. Results carry source IDs/revisions and embedding model/version. They fit within
the remaining inference budget and are supplied as data in `semantic_memory`. Scoped
runs persist read dependencies before inference. Source deletion, revision changes, or
permission withdrawal hide dependent run journals and pause the next execution boundary.
Existing remote grants remain disabled for retrieval. Scoped remote execution can
explicitly require Home retrieval as described below.

Generated Agents additionally need an approved, immutable `embedding` Registry provider
in their pinned generation policy, matching the workspace index configuration exactly.
Every generated ancestor supplies call allowance and shared token budget for both query
embeddings and background memory indexing. Live authority, expiry, catalog approval and
`embedding.invoke` are rechecked before each call. Failed or interrupted attempts remain
charged, and unapproved providers are rejected before disclosure. The bilingual Registry
and generation forms expose this contract; see [approved embeddings](generation.md#approved-embeddings).

## Scoped remote execution

Home Node A owns the Task, Workspace, semantic sources, index, embedding provider
and Qdrant configuration. Execution Node B runs the exact admitted Agent version.
Create the Home grant with `semantic.mode: "required_home"` and the approved Home
embedding Registry reference, then activate it through the existing scoped execution
API. The dashboard's **Assign agent** form exposes this selection; retrieval is
initially disabled. An explicit compactor reference is optional and belongs to B.

```json
{
  "id": "c46f3cba-41fa-4009-b2d5-9267fdf6e48a",
  "node_id": "aidash://execution",
  "agent": { "id": "research", "version": "1.0.0" },
  "ttl_seconds": 3600,
  "semantic": {
    "mode": "required_home",
    "embedding": { "id": "home-embedding", "version": "1.0.0" },
    "compactor": { "id": "approved-compactor", "version": "1.0.0" }
  }
}
```

The grant pins the index revision and digest, complete embedding configuration,
execution model, optional compactor and generated ancestry at both nodes. Home
requires `semantic.disclose` in addition to search/read authority. B requires
`semantic.use` under its mapped subject and exact executor. Both peers must
advertise the supported scoped semantic protocol. Provider credentials remain at
the node that dispatches the provider request; no subject bearer is forwarded.

Before each task inference, B fixes its query inputs and operation ID durably. A
searches authorized shared sources and sources scoped to **B's node/id/version**.
It does not search B's local store. Source-class controls, tenant/workspace scope,
metadata, current read authority, context budget and provider contract all apply.
A persists consumed source IDs, revisions and digests; B persists the receipt
before supplying its text to compaction or inference. Empty, truncated and ready
outcomes are distinct. Empty retrieval makes no query embedding request. Replaying
an empty receipt rechecks the candidate set. Old vectors cannot provide text.

Both nodes recheck current authority before disclosure, provider use, output adoption
and subsequent content delivery. The current reader needs a direct subject mapping
at every authority node; the producer's credential never substitutes for that reader.
Node-qualified dependency traversal detects cycles and has a 256-edge limit and a
20-second total transport deadline. An unavailable authority withholds content.
Installation-operator views cannot supply this tenant identity, so content views of
workspaces bound to required Home retrieval require a Subject credential. Minimal
Run pause/cancel metadata remains available without exposing a journal or source.

Retrieval is read-only: remote `memory_write` is neither advertised nor accepted.
Home ingestion retains its own initiating identity and generated budget. Compaction
uses only the explicitly selected B provider, with every generated ancestor's
approval; there is no environment-selected fallback for this remote path.

Temporary retrieval failures persist retry times: the initial attempt is followed
by at most five retries after 2, 4, 8, 16 and 32 seconds. Exhaustion pauses the Run.
Configuration, authority, budget and provider-contract failures pause immediately.
Explicit Home retry starts another bounded cycle only if the original binding is
still valid; it does not renew a grant or generation lifetime. Dispatched attempts
are never replayed, and uncertain provider charges remain reserved. See
[remote generation accounting](generation.md#foreign-tasks-and-origin-owned-allowances).

Changing or deleting a consumed source invalidates the original Run and hides its
derived journals, Messages, Artifacts, events and semantic re-ingestion at both nodes.
Dependencies survive context pruning and summaries. The Home dashboard offers an
explicit **Create follow-up task** action with fresh instructions; it creates a new
Task and does not copy invalid context. It cannot resume the invalidated Run.

Home execution summaries are under `GET /api/tasks/{task}/remote-executions`.
Authorized provenance is under `GET /api/tasks/{task}/remote-grants/{grant}/semantic`
and `GET /api/runs/{run}/semantic` at B. These expose bounded source references and
the serving node's authoritative generation counters, without source text. B's
`GET/POST /api/runs/{run}/management` supports content-free pause/cancel only.
Japanese and English views clear denied provenance on refresh and show typed pause
reasons, retry times, local allowance consumption and appropriate controls.

This is the selected combined FR-MEM-001 / section-12 scenario; every remote Agent
is not required to search. Other integrated release gates remain separate. The
[accepted specification](design/2026-09-30-remote-semantic-memory-specification.md)
and [verification ledger](design/2026-09-30-remote-semantic-memory-implementation.md)
record the contract and observed results.

## Recovery and visibility

Both server and worker processes run the durable indexer. `PENDING`, `READY`, `ERROR`,
`REVOKED`, and `DELETED` are distinct. Failures retain sanitized diagnostics and retry
with bounded exponential delays. Jobs retain their initiating credential and subject
chain, rechecking both before embedding. Restored authority or a linked-source update
allocates a new revision/point. Fresh processes resume persisted work.

Model/configuration changes retire the previous collection and reindex live sources.
Queries wait with HTTP 409 while authorized candidates are incomplete. Disabled indexes
also reject search. Embedding/vector transport failures, invalid output, and missing
acknowledged points return HTTP 503, without the distributed-transaction pending header.
An empty authorized candidate set returns an empty result without contacting providers.
There is no silent keyword fallback. Periodic verification rebuilds missing physical
points; explicit reindexing can request recovery immediately.

Upserts and deletes wait for Qdrant acknowledgement with strong ordering; reads request
all-replica consistency. A SQL tombstone removes an ID from retrieval immediately.
Retired point/collection identities remain in PostgreSQL and are deleted repeatedly,
including after an earlier request timed out and later arrived. Cleanup failures remain
visible through the cleanup endpoint. Back up PostgreSQL along with Qdrant; restoring a
stale PostgreSQL authority snapshot is an operator recovery decision, not an automatic
permission rollback performed by the indexer.

## Verification

`scripts/test-rust.sh` starts PostgreSQL, NATS, and Qdrant. The semantic tests use a local
OpenAI-contract embedding fixture with explicitly related vectors and real Qdrant
storage. They cover non-keyword retrieval plumbing, source/model revisions, ingestion
replay, filters, source authority, scoped Agent context, deletion, credential revocation,
missing points, backend outages, and reconstruction with fresh database pools. One test
starts its own disposable Qdrant container and restarts its process with persisted storage;
it never restarts the shared development service. The browser suite exercises the bilingual
management workflow and responsive layout. These fixtures verify integration behavior;
they do not measure the quality of a commercial embedding model.

Provider contracts: [Qdrant points](https://api.qdrant.tech/api-reference/points/upsert-points),
[Qdrant query](https://api.qdrant.tech/api-reference/search/query-points),
[OpenAI embeddings](https://developers.openai.com/api/reference/resources/embeddings/methods/create).
