# Protocol and recovery contracts

This document describes the implemented runtime and recovery contracts. Cross-node atomic transactions, semantic vector memory, orchestration and full A2A interoperability remain under development.

## Trust and identity

A node publishes `/.well-known/aidash` with its `aidash://` identity, endpoint, capabilities, clusters and protocol version. All management routes under `/api` require a bearer access token except the public, credential-free `/api/openapi.json` contract. The dashboard uses authenticated fetch for SSE so credentials do not appear in URLs.

The operator token retains privileged bootstrap, legacy operation and recovery access. Revocable subject tokens apply the current tenant policy to workspace and conversation APIs, human interaction, approved Registry discovery, local execution, state collections and event delivery. Local runs persist the root credential and delegated agent chain, rechecking their intersected authority at each durable boundary. See [authorization](authorization.md) for actions, catalog approvals, revocation and credential rotation. Scoped remote delegation and legacy admission into scoped workspaces remain closed pending cross-node identity integration.

Federation uses `/federation/v0.1`. Each request must include `Authorization: Bearer <peer credential>`, `X-Aidash-Node`, and `X-Aidash-Protocol: 0.1`. A peer must already be enabled in the receiving node's database. Peer credentials must have at least 32 printable ASCII characters and eight distinct characters, checked during registration and use. Generate a random token for each trust relationship. Each node has its own credential references; no central identity or message broker is required across nodes. In deployment, use HTTPS and trusted operator-managed tools. This version is not a hostile multi-tenant sandbox.

| Endpoint          | Purpose                                                                         |
| ----------------- | ------------------------------------------------------------------------------- |
| `POST /discover`  | Search locally registered agent metadata; does not recursively broadcast        |
| `POST /offers`    | Accept an idempotent task offer and persist a local run                         |
| `POST /workspace` | Claim, read or modify the offered task's home workspace                         |
| `GET /observe`    | Return runs, invocations and human requests belonging to the caller's home node |
| `POST /control`   | Pause/resume/cancel, deliver messages or answer human requests for those runs   |

The home node persists a delegation grant before sending an offer. Grants are scoped to the task, peer and exact agent version. Retrying an offer cannot replace its executor. Claims enforce capability requirements, dependency completion, `OPEN` status and the expected revision in one SQL update. A task's qualified owner is derived from the authenticated peer, never accepted as an arbitrary caller-supplied identity. Unknown task requirement/search fields are rejected. A terminal task grant permits snapshot/task reads, exact idempotent completion replay, or acknowledgment of the same terminal transition; it no longer authorizes workspace mutations.

Remote workers retrieve workspace metadata with `snapshot_workspace`, then use `snapshot_page` for the `tasks`, `artifacts`, `events` and `messages` collections. Each page has `items` and an optional UUID `next` cursor, supplied as `after` on the next request. Pages contain at most 32 records and 3 MiB of serialized data, below the 4 MiB peer decoder limit. Tasks and artifacts are paginated to completion; events and messages retain their latest-100 observation window. An individual resource larger than 3 MiB is rejected explicitly. Collection reads observe current home state and do not provide a transaction snapshot across pages.

Agent memory is separated by agent ID, version, workspace UUID and workspace home node. Existing local memory retains its local namespace; a remote offer cannot read or overwrite it by reusing a workspace UUID. A dependent run fails with the dependency ID and terminal status when a prerequisite is failed, cancelled or abandoned.

## Persistence and delivery

API startup and PostgreSQL worker recovery do not wait for NATS. The event-bus supervisor reconnects independently; federation offer retries also run independently of the broker.

A state mutation and its event are committed together. The event table doubles as an outbox. Its publisher waits for a JetStream acknowledgment, then records publication. A crash in between can replay an event, using its event ID as `Nats-Msg-Id`. Inbox uniqueness and task/run uniqueness remain the permanent deduplication boundary after JetStream's short duplicate window expires.

When a CloudEvent plus its NATS headers exceeds the broker payload limit, the publisher omits `data` and includes a `dataref` URI such as `/api/events?after=123`. Resolve this URI against the source node using an authorized API credential and match the event ID in the returned replay page. All CloudEvent identity and sequence fields remain intact; the full event stays in PostgreSQL. Small events retain their inline `data`.

Event sequence allocation is serialized through a transaction advisory lock so commit order agrees with the SSE cursor. `/api/events/stream` honors `Last-Event-ID`; `/api/events?after=N` provides JSON replay. A disconnected or slow client can resume from the PostgreSQL event log. SSE is an observation channel, not the worker's durable queue.

The workspace's home node owns task revisions and artifacts. A remote node owns its run journal and tool invocations. Federation commands operate on the home API and use stable keys. When a reply is lost, replay either returns the original result or reports a conflict for mismatched input. This currently provides no cross-node atomic transaction; FR-TX-001 requires that additional protocol and its failure/recovery verification before v0.1.0 is complete.

## Execution and effects

Workers lease one run at a time with a random lease token and a 30-second expiry. They renew every 10 seconds and fence state and tool-result writes by token and expiry. A stale worker cannot advance a new owner's journal. A process crash leaves the last persisted phase, model response and tool-call cursor available for recovery.

The tool invocation record is committed before sending the request. Each tool call has a key derived from run ID, inference step and call index. Its result is committed before advancing that cursor. Replayed completed invocations return the stored result. Task completion and its final artifact are one transaction with an idempotency key and input consistency checks.

For an external effect, exactly-once behavior requires cooperation from the target. An HTTP tool declared `idempotent` must honor `Idempotency-Key`; MCP tools must honor the configured idempotency argument. Declaring an adapter idempotent does not make an arbitrary server idempotent. For `unsafe` adapters, a missing result after an attempt is treated as uncertain and the effect is not repeated. An operator verifies the external outcome and answers the confirmation with `{"result": ...}`. This restores the journal without executing the effect again.

Controls are cooperative at persisted step boundaries. Pause lets an in-flight call finish and persist before stopping the next step. Cancel cannot undo an already performed effect. Human questions and answers are durable. A normal human answer returns the agent to inference; remaining tool calls from the pre-question response are discarded, so a rejected approval cannot release a queued effect.

Transient execution errors have a bounded retry budget per persisted operation. A successful operation resets the counter. Exhausted or permanent failures persist a `terminal_transition` intent in a waiting run. The worker retries home-node delivery across outages and restarts and only finalizes its local journal after the home acknowledges that outcome. It does not re-execute the failed tool while delivering this intent.

A coordinator's final response remains gated while a child is unresolved. An operator can call `POST /api/tasks/{id}/abandon` with the current revision and a nonempty reason for a failed, blocked, or cancelled child. Nested children must be completed or abandoned first. The home records `ABANDONED` with the prior status and reason; a resumed parent can finish with remaining artifacts. Dependencies still require actual completion: abandonment does not satisfy a task dependency.

### Context compaction

The compaction trigger, post-compaction fit check and final preflight use the
same complete-request estimate: UTF-8 bytes of the serialized model-visible
messages and tool definitions, plus `max_output_tokens`, plus 1,024 framing
tokens. This deliberately conservative estimate is not the provider's actual
token count. It includes the JSON escaping of context inside message text.
Output fits inside the registered context window; it is not extra input space.
Failure to fit leaves saved context unchanged and prevents inference I/O.

The model's pinned workspace and `workspace_observe` use the same bounded view.
Observations include goal/task previews, artifact IDs and metadata, message
previews, and event IDs/kinds/timestamps. Event payloads are never automatically
embedded, so observing an earlier `tool.completed` event cannot copy its full
observation result into the next request. Durable snapshots, invocation records
and audit APIs retain their original content.

`workspace_observe` accepts optional `offset` (default 0) and `limit` (default 20,
maximum 50). Each collection reports its total and next offset. Events/messages
are newest first; tasks/artifacts retain snapshot order. These are live pages,
so refresh after concurrent changes. `workspace_read` selects an accessible
`workspace`, `task`, `artifact`, `message` or `event` by exact ID, through the
same authorization-filtered Home snapshot. It returns JSON text in Unicode
character ranges: `offset`, `max_chars` (default 8,000, maximum 16,000),
`total_chars` and `next_offset`. Concatenate chunks in offset order to recover
the full record. Missing and inaccessible IDs return the same error. Reading
mutable records across concurrent changes requires restarting the read.

On replay, legacy `workspace_observe` results are projected into this view in
the working context. This changes only observation representation; human
records, other tool results and the underlying audit journal are preserved.
The conversion commits only when the complete request fits.

Context compaction adapts [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction/tree/e3f262a7f4d42bd8dd32ced30d26176f7cb545b0) to Aidash's paired tool events. Jev receives a fitted classification view and returns separate call/result retention probabilities; it does not rewrite human messages or summarize history. The first and recent events, non-tool text, instructions, current task/workspace, memory, and existing summaries remain verbatim in the inference context. Unneeded pairs can be dropped and unneeded outputs truncated, while complete invocation results remain in PostgreSQL. Every batch sees the same fitted state, with bounded request size and four concurrent requests. Invalid answers, missing credentials or insufficient compaction leave the original context unchanged. Compaction uses `AIDASH_SECRET_JEV`, with endpoint/model overrides in `.env.example`, independently of the explicitly selected inference model. Token estimates are conservative and are not provider tokenization.

The default retention threshold is 0.5: keep the complete pair when the result probability reaches the threshold; otherwise retain the call with the first 300 result characters and a notice when the call probability reaches it; otherwise drop the pair. Results up to 420 characters are unchanged. The first and latest six history events are pinned.

The classifier state is limited to 25,000 estimated tokens. Tool input caps shrink through 1,000/200/60 characters, followed by text abridgment, old-text size notes, compact call lines, omission of old call-less entries and merging of adjacent old call-only entries. These reductions never alter stored human or assistant text. Questions are divided into requests of at most 30,000 estimated tokens. The Jev estimator counts letter runs, digits and symbols following upstream; inference uses a separate conservative character-based budget.

The default endpoint is `https://api.typesafe.ai/v1/systemone` and the default model is `jev-latest`. Bearer-authenticated requests send `{model,state,questions}`. The upstream MIT notice is included in [LICENSE](../LICENSE).

## Marketplace and localization

The node is a self-hosted package repository. A manifest includes the versioned entity, author, permissions and dependencies. Publication is immutable. Installation verifies the SHA-256 digest, validates dependencies and atomically registers the entity with local installation configuration. This version distributes declarative manifests; it does not execute downloaded code, operate a public SaaS marketplace or handle payments.

UI strings live in `web/src/locales/en-US.json` and `ja-JP.json`. Entity names and descriptions are localized maps. Agent language capabilities are separate metadata used by local and remote discovery. Message translation is not automatic.

### Bounded inspection and retry identifiers

`GET /api/tasks?offset=N` returns up to 500 authorized tasks, newest first, with
`next_offset` for the next page. `/api/state` includes only the first task page.
`GET /api/runs/{id}?offset=N` returns at most 100 invocation previews; request the
next offset when a page is full. Inputs/results over 1 KiB are represented by an
explicit `truncated` marker and text preview. Durable execution records retain
original values. Credential inventories use `?offset=N` pages of 200 records.
Scoped collections fill pages after authorization checks.

Federation discovery returns `entries` and `next_offset`, with at most 64 entries
and 3 MB of metadata per response. Exact delegation validation uses
`GET /federation/v0.1/discover/{id}/{version}`. Generated task-bound agents are
excluded from these legacy discovery routes. Observation returns at most 100
runs, human requests and invocation previews; run context and pending payloads
remain available through the authorized run detail API.

Run-message and remote message controls accept a UUID `idempotency_key`. Reuse
that key when retrying an ambiguous request. The dashboard keeps it until that
submission succeeds. A stream request with `Last-Event-ID: -1` starts from the
current high-water mark; the dashboard refreshes state after the connection is
established and then resumes from acknowledged event IDs on reconnect.

Optional workspace snapshots are reduced to the selected model's available
context budget before history compaction, with a `snapshot_truncated` marker.
The durable workspace remains intact and tools can retrieve omitted details.
Automatic semantic retrieval skips budgets too small to contain provenance.

Agent registration rejects instructions, referenced skills and tool definitions
that cannot fit the selected model window with output and context reserves.
