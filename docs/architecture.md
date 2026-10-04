# Aidash 0.1 architecture

This document describes the current implementation and its boundaries. Scoped remote execution has an activation and Home-command path, but its complete two-node acceptance remains open (#38). Complete A2A compatibility (#39), transaction release acceptance (#40), and the combined six-capability release gate also remain open.

## Current implementation

The implementation is a Rust 2024 Cargo workspace, with independent control server
and worker modes and a React/TypeScript dashboard. Domain models and pure rules
live in `aidash-domain`; application use cases and external ports live in
`aidash-application`; `aidash-runtime` supervises background work; and
`aidash-integrations` implements external connections. `aidash-server` owns
Reinhardt HTTP, ORM repositories, settings, migrations, and dependency assembly.
HTTP requests and workers use the same bootstrap and persistence implementations.

Worker entry, inference admission, scoped command replay and capability operation
admission use application ports. Transaction manifests and monotonic coordinator
decisions are domain rules; transaction authority completes inherited subject
chains before checking mutations and recipient disclosure. Native scopes retain
the caller's transaction and lock order. Authority withdrawal reloads the Area
before the operation under row locks, keeps cancellation possible after
revocation, and retains possible effects for dispatched writers.

Coordinator advancement, decision verification, operator abort and recovery
batches use application ports. Domain rules select participant votes and bind
acknowledgements to the immutable manifest and exact phase. Native repositories
retain the recovery advisory lease across participant I/O, update each vote and
clear its error atomically, and commit immutable decisions with their audit
history under the same row lock. Operator abort competes for that decision
without waiting for the recovery lease.
Runtime schedules active and aborted recovery independently; bootstrap supplies
their separate connection capacity before the supervisor starts the loops.
Manifest admission uses a borrowed application scope over the caller's existing
transaction. Immutable replay checks the bound origin before accepting the
manifest; new admission checks the deadline and peer trust before persisting
the coordinator, votes, history and origin binding.
Participant reserve, prepare, finish and recovery also use application scopes.
Durable reservations precede live authorization rechecks. Native scopes retain
serializable transactions, speculative validation savepoints, mutation context,
visibility barriers and authority auditing. Commit application and visibility
release remain separate durable transitions; abort tombstones reject delayed
reservation replay.
Participant recovery is also scheduled by runtime under the drain supervisor;
server services compose the same application workflow for explicit recovery.
Application also orders manifest mutations, task child/delegation checks, paired
task and Run completion, provenance recording and their unchanged event payloads.
Domain rules check task and Run preconditions before execution-state decoding.
Native mutation ports retain row locks, database-clock lease checks, selective
updates and event persistence inside the participant's existing transaction;
speculative preparation runs this same workflow under its rollback-only savepoint.
Transaction HTTP replies are decoded in integrations. Current peer lookup,
response headers and the complete body share the original ten-second deadline;
the four-MiB limit, rotating credentials and recovery error classification remain
unchanged for coordinator and authority RPCs.
Authorized subject aborts also arbitrate and inspect the immutable decision
through application ports inside the retained authority transaction. A concurrent
commit remains irrevocable, and fault cuts retain their before/after commit order.
Transaction management uses application disclosure and trust workflows. Ordered
keyset pages retain eight concurrent live-authority checks and the 200-visible-row
limit. Native adapters keep votes/audit on the same connection and retain trust
registration through the post-commit pending scan; HTTP services only compose
authentication metadata and unchanged response contracts.

Durable worker activation uses application scheduling and handoff ports. Targeted
notifications and recovery share database-clock eligibility, malformed-state
repair and revision/lease fencing. Native repositories retain Run-before-obligation
lock ordering and commit execution responsibility before any acknowledgement.
An ACK failure still advances the committed lease; invalid references enter
durable quarantine before TERM. Runtime owns reconnect backoff, bounded publication,
finite single-message pulls, recovery cadence and drain. Integrations owns JetStream
stream/consumer validation, credential decoding and transport acknowledgements;
bootstrap supplies the same ports to server and listener-free worker modes.

Worker admission and terminal delivery use application scopes that retain fresh
authority through their effects. Runtime owns lease heartbeats, transient renewal
backoff, cancellation observations and terminal-outbox polling. A lost lease
cancels the current operation without recovery or settlement; a completed step
resumes ordinary visibility before recovery reads its committed state. Native
adapters project only the committed run identifier or control for observations
and preserve the existing failure-delivery transaction and lease fence.

Scoped commands authorize effects, track disclosed outputs, journal mutation
results, and recheck the source lease through the application use case. A claim
binds the complete inspected agent definition. Operation reconciliation owns one
authority scope through commit or rollback, records attempted dispatch before
contacting the runner, and publishes outputs atomically before advancing its
revision. Runner HTTP lives in integrations; its transport restrictions,
credential rotation, timeout and verified deployment ceilings retain the existing
execution contract.

Capability sessions, explicit retention and restoration, reference extraction,
approval decisions, Python lifecycle, file operations and Skill loading use
application ports over the same native authority transaction. Patch publication
checks every preimage before exposing the new manifest. Reference and cleanup
workers require a confirmed writer stop before reclaiming bytes; runtime owns
their scheduling and drain loops. Outbound HTTP records an attempt before the
integration transport contacts the remote service. Current subject chains and
source constraints apply to both HTTP and worker entry points.

Authoritative state lives in PostgreSQL; Qdrant stores derived semantic vectors.
Reinhardt owns the single migration graph under `server/migrations/`. Its frozen
baseline retains PostgreSQL functions, triggers, generated columns, constraints,
indexes, and lock/lease semantics from the 54-step development schema. State-only
ORM snapshots support future autodetection without replacing these physical
guarantees. Native repository operations retain the caller's transaction and
visibility/authority scope; transaction control uses separate connection capacity.
The [migration runbook](../server/migrations/README.md) describes the supported
empty-database boundary and maintenance cutover. Runtime queries are tested with
real PostgreSQL; builds do not require a live database.

The workspace's home node owns its task revisions, messages and artifacts. Remote agents claim and mutate these resources through the versioned HTTP federation protocol. They never connect to the home database. Each executing node persists its own run journal. Peer trust is explicitly configured on both nodes with environment-based credential references. Public identity contains no secrets. Registry versions are immutable and model selection is explicit.

Every execution step is persisted before advancing. A leased worker processes one step at a time, renews the lease during inference/tool work and fences journal writes against stale owners. Tool calls are journaled before invocation. Read-only and idempotent adapters may replay with the original key. An unsafe invocation whose outcome was not committed is blocked for human reconciliation, never silently repeated. An arbitrary external service cannot offer exactly-once effects without cooperating with the idempotency contract.

State mutations append durable events in the same transaction. An outbox publisher waits for JetStream persistence before marking an event sent. Consumer acknowledgments follow a committed inbox record; redelivery is deduplicated. PostgreSQL scanning also recovers runnable work after restart, including a publication outage. SSE resumes from a durable event sequence and uses the database log, so reconnection does not depend on an in-memory broadcast buffer.

The marketplace is a self-hosted, versioned manifest repository exposed by the node API. Installation validates metadata and dependencies, verifies the digest and atomically registers the entity with local configuration. Payments, rankings, WASM sandboxing and automatic model routing remain outside this version. Kubernetes/k3s orchestration, automatic agent generation, complex RBAC/ABAC, distributed transactions, semantic memory/vector DB and full A2A compatibility are required for v0.1.0. Several protocol and runtime paths exist, but their integrated release acceptance is incomplete. The [generation backend](generation.md) connects revisioned policies and durable quota reservations to atomic registration, scoped worker execution and lifecycle reconciliation.

## Baseline implementation and verification sequence

1. Define entities, schema validation, PostgreSQL migrations and task transitions.
2. Implement providers, tool adapters, context compaction and the durable worker.
3. Connect event delivery, federation, interaction and marketplace APIs.
4. Build the localized dashboard on those APIs, including task ownership, mesh, execution and human controls.
5. Verify with real PostgreSQL and NATS, scripted provider protocol fixtures, two independent nodes, concurrent claims, redelivery and worker process termination/restart. Live commercial model quality is separate from protocol and recovery testing.

## Required architecture extensions

The v0.1.0 acceptance must cover orchestration without losing durable node/run identity, policy-controlled agent creation, shared RBAC/ABAC enforcement, a recoverable atomic transaction protocol between participating nodes, authorized semantic retrieval and an A2A v1.0.0 client/server boundary. Keep explicit model selection, independently operated node databases and the existing durable tool-effect contract. Cross-node transactions communicate through participating node APIs instead of accessing remote databases directly.

The scoped remote activation and Home-command path does not by itself satisfy every FR-AUTH-001 acceptance case; see [authorization](authorization.md) and #38. The [distributed transaction protocol](transactions.md) implements durable prepare/commit/abort and cross-node visibility barriers; its full FR-TX-001 authorization and failure-acceptance gate remains open. [Semantic memory](semantic-memory.md) adds Qdrant indexing, authorized retrieval and provenance in local Agent context and explicitly admitted Home-owned remote context. [Kubernetes/k3s orchestration](orchestration.md) adds independently scalable roles, graceful shutdown and deployment observations. Their cross-capability acceptance remains a separate release gate. Aidash federation endpoints do not establish A2A compatibility.
