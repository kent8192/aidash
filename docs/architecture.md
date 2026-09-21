# Aidash 0.1 architecture

This document describes the current implementation and its boundaries. Scoped federation, complete A2A compatibility and the combined six-capability release acceptance remain under development.

## Current implementation

The implementation is a Rust node executable, with independent control server and worker modes, and a React/TypeScript dashboard. All state lives in PostgreSQL. SeaORM owns schema migrations and registry CRUD; SQLx executes transactional task transitions, execution journals, and the event outbox. Application and test queries use SeaQuery builders. PostgreSQL trigger functions, triggers and ALTER CHECK operations that SeaQuery cannot represent remain explicit SeaORM migration DDL. They share the application data pool; transaction control and visibility leases use separate connection capacity. SQLx runtime queries are exercised against real PostgreSQL so builds do not require a live database.

The workspace's home node owns its task revisions, messages and artifacts. Remote agents claim and mutate these resources through the versioned HTTP federation protocol. They never connect to the home database. Each executing node persists its own run journal. Peer trust is explicitly configured on both nodes with environment-based credential references. Public identity contains no secrets. Registry versions are immutable and model selection is explicit.

Every execution step is persisted before advancing. A leased worker processes one step at a time, renews the lease during inference/tool work and fences journal writes against stale owners. Tool calls are journaled before invocation. Read-only and idempotent adapters may replay with the original key. An unsafe invocation whose outcome was not committed is blocked for human reconciliation, never silently repeated. An arbitrary external service cannot offer exactly-once effects without cooperating with the idempotency contract.

State mutations append durable events in the same transaction. An outbox publisher waits for JetStream persistence before marking an event sent. Consumer acknowledgments follow a committed inbox record; redelivery is deduplicated. PostgreSQL scanning also recovers runnable work after restart, including a publication outage. SSE resumes from a durable event sequence and uses the database log, so reconnection does not depend on an in-memory broadcast buffer.

The marketplace is a self-hosted, versioned manifest repository exposed by the node API. Installation validates metadata and dependencies, verifies the digest and atomically registers the entity with local configuration. Payments, rankings, WASM sandboxing and automatic model routing remain outside this version. Kubernetes/k3s orchestration, automatic agent generation, complex RBAC/ABAC, complete distributed transactions, semantic memory/vector DB and full A2A compatibility are required for v0.1.0 and remain under implementation. The [generation backend](generation.md) now connects revisioned policies and durable quota reservations to atomic registration, scoped worker execution and lifecycle reconciliation.

## Baseline implementation and verification sequence

1. Define entities, schema validation, PostgreSQL migrations and task transitions.
2. Implement providers, tool adapters, context compaction and the durable worker.
3. Connect event delivery, federation, interaction and marketplace APIs.
4. Build the localized dashboard on those APIs, including task ownership, mesh, execution and human controls.
5. Verify with real PostgreSQL and NATS, scripted provider protocol fixtures, two independent nodes, concurrent claims, redelivery and worker process termination/restart. Live commercial model quality is separate from protocol and recovery testing.

## Required architecture extensions

Add orchestration without losing durable node/run identity, policy-controlled agent creation, shared RBAC/ABAC enforcement, a recoverable atomic transaction protocol between participating nodes, authorized semantic retrieval and an A2A v1.0.0 client/server boundary. Keep explicit model selection, independently operated node databases and the existing durable tool-effect contract. Cross-node transactions must communicate through participating node APIs instead of accessing remote databases directly.

The current bearer token and scoped delegation grants do not satisfy FR-AUTH-001. The [distributed transaction protocol](transactions.md) adds durable prepare/commit/abort and cross-node visibility barriers; its full FR-TX-001 authorization and failure-acceptance gate remains open. [Semantic memory](semantic-memory.md) adds Qdrant indexing, authorized retrieval and provenance in local Agent context. [Kubernetes/k3s orchestration](orchestration.md) adds independently scalable roles, graceful shutdown and deployment observations. Their cross-capability acceptance remains a separate release gate. Aidash federation endpoints do not establish A2A compatibility.
