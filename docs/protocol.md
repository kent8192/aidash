# Protocol and recovery contracts

## Trust and identity

A node publishes `/.well-known/aidash` with its `aidash://` identity, endpoint, capabilities, clusters and protocol version. All management routes under `/api` require a bearer access token. The dashboard uses authenticated fetch for SSE so credentials do not appear in URLs.

Federation uses `/federation/v0.1`. Each request must include `Authorization: Bearer <peer credential>`, `X-Aidash-Node`, and `X-Aidash-Protocol: 0.1`. A peer must already be enabled in the receiving node's database. Each node has its own credential references; no central identity or message broker is required across nodes. In deployment, use HTTPS and trusted operator-managed tools. This version is not a hostile multi-tenant sandbox.

| Endpoint          | Purpose                                                                         |
| ----------------- | ------------------------------------------------------------------------------- |
| `POST /discover`  | Search locally registered agent metadata; does not recursively broadcast        |
| `POST /offers`    | Accept an idempotent task offer and persist a local run                         |
| `POST /workspace` | Claim, read or modify the offered task's home workspace                         |
| `GET /observe`    | Return runs, invocations and human requests belonging to the caller's home node |
| `POST /control`   | Pause/resume/cancel, deliver messages or answer human requests for those runs   |

The home node persists a delegation grant before sending an offer. Grants are scoped to the task, peer and exact agent version. Retrying an offer cannot replace its executor. Claims enforce capability requirements, dependency completion, `OPEN` status and the expected revision in one SQL update. A task's qualified owner is derived from the authenticated peer, never accepted as an arbitrary caller-supplied identity.

## Persistence and delivery

A state mutation and its event are committed together. The event table doubles as an outbox. Its publisher waits for a JetStream acknowledgment, then records publication. A crash in between can replay an event, using its event ID as `Nats-Msg-Id`. Inbox uniqueness and task/run uniqueness remain the permanent deduplication boundary after JetStream's short duplicate window expires.

Event sequence allocation is serialized through a transaction advisory lock so commit order agrees with the SSE cursor. `/api/events/stream` honors `Last-Event-ID`; `/api/events?after=N` provides JSON replay. A disconnected or slow client can resume from the PostgreSQL event log. SSE is an observation channel, not the worker's durable queue.

The workspace's home node owns task revisions and artifacts. A remote node owns its run journal and tool invocations. Federation commands operate on the home API and use stable keys. When a reply is lost, replay either returns the original result or reports a conflict for mismatched input. This is not a cross-node distributed transaction.

## Execution and effects

Workers lease one run at a time with a random lease token and a 30-second expiry. They renew every 10 seconds and fence state and tool-result writes by token and expiry. A stale worker cannot advance a new owner's journal. A process crash leaves the last persisted phase, model response and tool-call cursor available for recovery.

The tool invocation record is committed before sending the request. Each tool call has a key derived from run ID, inference step and call index. Its result is committed before advancing that cursor. Replayed completed invocations return the stored result. Task completion and its final artifact are one transaction with an idempotency key and input consistency checks.

For an external effect, exactly-once behavior requires cooperation from the target. An HTTP tool declared `idempotent` must honor `Idempotency-Key`; MCP tools must honor the configured idempotency argument. Declaring an adapter idempotent does not make an arbitrary server idempotent. For `unsafe` adapters, a missing result after an attempt is treated as uncertain and the effect is not repeated. An operator verifies the external outcome and answers the confirmation with `{"result": ...}`. This restores the journal without executing the effect again.

Controls are cooperative at persisted step boundaries. Pause lets an in-flight call finish and persist before stopping the next step. Cancel cannot undo an already performed effect. Human questions and answers are durable. A normal human answer returns the agent to inference; remaining tool calls from the pre-question response are discarded, so a rejected approval cannot release a queued effect.

Context compaction is inspired by [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction): tool calls and their results remain paired; selected historical events, recent events and human responses are retained verbatim. The selected model produces a summary and retention indices. Instructions, current task/workspace and memory remain pinned. Invalid or insufficient compaction fails visibly instead of silently dropping human constraints. Token estimates are conservative and are not provider tokenization.

## Marketplace and localization

The node is a self-hosted package repository. A manifest includes the versioned entity, author, permissions and dependencies. Publication is immutable. Installation verifies the SHA-256 digest, validates dependencies and atomically registers the entity with local installation configuration. This version distributes declarative manifests; it does not execute downloaded code, operate a public SaaS marketplace or handle payments.

UI strings live in `web/src/locales/en-US.json` and `ja-JP.json`. Entity names and descriptions are localized maps. Agent language capabilities are separate metadata used by local and remote discovery. Message translation is not automatic.
