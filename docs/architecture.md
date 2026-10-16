# Aidash 0.1 architecture

This document describes the current implementation and its boundaries. Scoped remote execution has an activation and Home-command path, but its complete two-node acceptance remains open (#38). Complete A2A compatibility (#39), transaction release acceptance (#40), and the combined six-capability release gate also remain open.

## Current implementation

The implementation is a Rust 2024 Cargo workspace, with independent control server
and worker modes and a React/TypeScript dashboard. Domain models and pure rules
live in `aidash-domain`; application use cases and external ports live in
`aidash-application`; `aidash-harness` owns agent execution and worker lifecycle;
`aidash-runtime` supervises process-wide background work; and
`aidash-integrations` implements external connections. `aidash-server` owns
Reinhardt HTTP, ORM repositories, settings, migrations, and dependency assembly.
HTTP requests and workers use the same bootstrap and persistence implementations.

The harness executes agent steps and owns worker activation, lease renewal,
cancellation, execution recovery and terminal delivery. Shared execution ports,
failure classifications and helpers remain in application/domain so those
crates never import the harness. Runtime's generic `Supervisor` accepts worker
futures without importing the harness; server composes both with native adapters.

Worker entry, inference admission, scoped command replay and capability operation
admission use application ports. Transaction manifests and monotonic coordinator
decisions are domain rules; transaction authority completes inherited subject
chains before checking mutations and recipient disclosure. Native scopes retain
the caller's transaction and lock order. Authority withdrawal reloads the Area
before the operation under row locks, keeps cancellation possible after
revocation, and retains possible effects for dispatched writers.

Coordinator advancement, decision verification, Deployment Administrator abort and recovery
batches use application ports. Domain rules select participant votes and bind
acknowledgements to the immutable manifest and exact phase. Native repositories
retain the recovery advisory lease across participant I/O, update each vote and
clear its error atomically, and commit immutable decisions with their audit
history under the same row lock. Deployment Administrator abort competes for that decision
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
durable quarantine before TERM. Harness owns reconnect backoff, bounded publication,
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

Remote run-message admission, reconciliation, Home reservation recovery and
observed-input acknowledgements share an application ledger scope. History import
and new-input acceptance remain one native transaction. A late connection error
checks the committed input before promoting or releasing its Home fence. Domain
rules bind delivered records to the original node, task, workspace and content;
typed native RPC adapters retain the preceding Federation 0.1 peer fallback and
its bounded history/snapshot behavior.

Dashboard account validity and status recovery are application use cases shared
by browser sessions and ongoing runs. Runtime drives bounded refresh passes;
integration adapters own OIDC discovery, JWKS caches and account lookups. Native
repositories retain conditional status writes, atomic session revocation and the
original Subject authority through resumed-run commits. Provider latency does
not extend validity: successful checks record their start time. Backchannel
logout verifies its signed envelope before applying domain lifetime and replay
identity rules and atomically consuming the logout identity with revocation.

Peer identity registration, bearer rotation checks and delegation authorization
use shared application rules. Native registration and trust restoration recheck
distinct credentials while retaining the same advisory lock, row locks and audit
transaction. Graph repositories retain projection queries and current viewer
leases; application checks workspace permission before record disclosure.

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

Authoritative state and derived memory search projections live in PostgreSQL.
pgvector performs semantic ranking; PGroonga provides multilingual keyword retrieval.
The native memory provider (Issue #125, merged through PR #130) is a Rust Hindsight
implementation on this backend. Qdrant is no longer an adapter or deployment service.
Existing JSON memory was discarded, with no legacy migration workflow. The
architecture decision for this backend is proposed under Issue #126 and awaits
the user's approval; the [traceability matrix](memory-traceability.md) records
the status of each memory requirement.

Reinhardt owns the single migration graph under `server/migrations/`. Its frozen
baseline retains PostgreSQL functions, triggers, generated columns, constraints,
indexes, and lock/lease semantics from the 55-step development schema. State-only
ORM snapshots support future autodetection without replacing these physical
guarantees. Native repository operations retain the caller's transaction and
visibility/authority scope; transaction control uses separate connection capacity.
Repository queries and transactions execute through Reinhardt's database
connection and transaction executors, including authorization leases and their
protected writes. Typed projections name tuple columns explicitly and distinguish
SQL NULL from JSON null. The underlying SQLx driver is confined to bootstrap pool
configuration and independent test fixtures; it does not execute repository
queries.
Host timestamps used by typed ORM updates and retention predicates are reduced
to PostgreSQL microseconds at the persistence boundary, preserving the previous
driver's truncation relative to its 2000-01-01 epoch. Business clocks retain their
original precision; outbox retries continue to use the database clock.
The [migration runbook](../server/migrations/README.md) describes the supported
empty-database boundary and maintenance cutover. Runtime queries are tested with
real PostgreSQL; builds do not require a live database.

The workspace's home node owns its task revisions, messages and artifacts. Remote agents claim and mutate these resources through the versioned HTTP federation protocol. They never connect to the home database. Each executing node persists its own run journal. Peer trust is explicitly configured on both nodes with environment-based credential references. Public identity contains no secrets. Registry versions are immutable and model selection is explicit.

Every execution step is persisted before advancing. A leased worker processes one step at a time, renews the lease during inference/tool work and fences journal writes against stale owners. Tool calls are journaled before invocation. Read-only and idempotent adapters may replay with the original key. An unsafe invocation whose outcome was not committed is blocked for human reconciliation, never silently repeated. An arbitrary external service cannot offer exactly-once effects without cooperating with the idempotency contract.

State mutations append durable events in the same transaction. An outbox publisher waits for JetStream persistence before marking an event sent. Consumer acknowledgments follow a committed inbox record; redelivery is deduplicated. PostgreSQL scanning also recovers runnable work after restart, including a publication outage. SSE resumes from a durable event sequence and uses the database log, so reconnection does not depend on an in-memory broadcast buffer. Tentative Inference Progress is stored apart from that journal with its own per-Run sequence and a 15-minute retention; the Run inference stream replays it the same way, and only accepted responses become durable Run state ([protocol](protocol.md#inference-progress)).

The Marketplace is a tenant-scoped, versioned repository exposed by the node API. Installation validates metadata and dependencies, verifies the digest and atomically registers an immutable tenant copy, which needs its own approval before use ([Marketplace operations](operations/marketplace-authorization.md)). Payments, rankings, WASM sandboxing and automatic model routing remain outside this version. Kubernetes orchestration, automatic agent generation, complex RBAC/ABAC, distributed transactions, semantic memory/vector DB and full A2A compatibility are required for v0.1.0. Several protocol and runtime paths exist, but their integrated release acceptance is incomplete. The [generation backend](generation.md) connects revisioned policies and durable quota reservations to atomic registration, scoped worker execution and lifecycle reconciliation.

### Memory contracts

Three contracts are separate ownership areas, each with its owning Issue. They share
mechanisms: Ordered retrieval reuse from #172 serves contracts (a) and (b), and the
mutation and recovery rules serve contracts (b) and (c). A change to a shared
mechanism needs regression and acceptance work in every contract that uses it.

- **(a) Run-context continuity and compaction.** Owned by #113 and #116. It covers how
  a Run's context survives restart and is reduced when it grows, through the Context
  Journal and Context Projection. Per-Run retrieval reuse for Ordered Runs is part of
  #172 (closed) and PR #186 (merged). This contract does not store memory.
- **(b) Persistent source ingestion and semantic retrieval.** Owned by #110, with
  retrieval reuse from #172 and PR #186. It covers Source ingestion, the semantic
  index on PostgreSQL with pgvector and PGroonga, and two retrieval paths that keep
  provenance, scope, lifecycle and authorization. Workspace Source retrieval searches
  the pgvector index (FR-MEM-001). Native memory recall is separate and ranks the
  authorized units of a Bank with four rankings (semantic, keyword, graph and
  temporal). A reranker is a required policy role: either a registered model or the
  explicit `Rrf` passthrough. A provider failure is an error, never a fallback to `Rrf`.
- **(c) Extraction, review and consolidation of reusable knowledge.** Owned by #118,
  with autonomous admission in #141 (open) and the reviewed native memory path in PR #130.
  It covers candidates from completed Runs, provenance-bound review, and promotion to
  canonical memory units.

The mutation rules and managed recovery of memory units (#125, PR #130) apply to
the units in contracts (b) and (c). Their status for each requirement, and the
evidence behind it, is in the [traceability matrix](memory-traceability.md).

### Registry boundary

A Registry descriptor (#110) is discoverable metadata for a capability, not its
content. For a native memory provider, the descriptor configuration holds only the
engine and policy. Supported operations, exact-version pinning, disclosure and
Bindings are separate Registry and admission controls (see
[Native Memory providers](operations/registry-capabilities.md#native-memory-providers)).
Private memory bodies and credentials are not part of a descriptor. Memory bodies
stay in the authorized provider storage of the Home, and credentials stay in the
Provider Credential Store. Catalog metadata never exposes either.

## Baseline implementation and verification sequence

1. Define entities, schema validation, PostgreSQL migrations and task transitions.
2. Implement providers, tool adapters, context compaction and the durable worker.
3. Connect event delivery, federation, interaction and marketplace APIs.
4. Build the localized dashboard on those APIs, including task ownership, mesh, execution and human controls.
5. Verify with real PostgreSQL and NATS, scripted provider protocol fixtures, two independent nodes, concurrent claims, redelivery and worker process termination/restart. Live commercial model quality is separate from protocol and recovery testing.

## Required architecture extensions

The v0.1.0 acceptance must cover orchestration without losing durable node/run identity, policy-controlled agent creation, shared RBAC/ABAC enforcement, a recoverable atomic transaction protocol between participating nodes, authorized semantic retrieval and an A2A v1.0.0 client/server boundary. Keep explicit model selection, independently operated node databases and the existing durable tool-effect contract. Cross-node transactions communicate through participating node APIs instead of accessing remote databases directly.

The scoped remote activation and Home-command path does not by itself satisfy every FR-AUTH-001 acceptance case; see [authorization](authorization.md) and #38. The [distributed transaction protocol](transactions.md) implements durable prepare/commit/abort and cross-node visibility barriers; its full FR-TX-001 authorization and failure-acceptance gate remains open. Semantic memory adds PostgreSQL indexing, authorized retrieval and provenance in local Agent context and explicitly admitted Home-owned remote context. [Kubernetes orchestration](orchestration.md) adds independently scalable roles, graceful shutdown and deployment observations. Their cross-capability acceptance remains a separate release gate. Aidash federation endpoints do not establish A2A compatibility.
