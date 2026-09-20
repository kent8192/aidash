# Aidash 0.1 architecture

Source: [v0.1.0 functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24), read 2026-09-20, last edited 13:33:55 UTC. The version-specific exclusions take precedence over the broader [technology selection](https://app.notion.com/p/3e172fa877aa80e88c60d0dfeae01419). The non-functional requirements page was empty.

The implementation is a Rust node executable, with independent control server and worker modes, and a React/TypeScript dashboard. All state lives in PostgreSQL. SeaORM owns registry CRUD; SQLx owns migrations, task transitions, execution journals, and the event outbox. Both share one pool. SQLx runtime queries are exercised against real PostgreSQL so builds do not require a live database.

The workspace's home node owns its task revisions, messages and artifacts. Remote agents claim and mutate these resources through the versioned HTTP federation protocol. They never connect to the home database. Each executing node persists its own run journal. Peer trust is explicitly configured on both nodes with environment-based credential references. Public identity contains no secrets. Registry versions are immutable and model selection is explicit.

Every execution step is persisted before advancing. A leased worker processes one step at a time, renews the lease during inference/tool work and fences journal writes against stale owners. Tool calls are journaled before invocation. Read-only and idempotent adapters may replay with the original key. An unsafe invocation whose outcome was not committed is blocked for human reconciliation, never silently repeated. An arbitrary external service cannot offer exactly-once effects without cooperating with the idempotency contract.

State mutations append durable events in the same transaction. An outbox publisher waits for JetStream persistence before marking an event sent. Consumer acknowledgments follow a committed inbox record; redelivery is deduplicated. PostgreSQL scanning also recovers runnable work after restart, including a publication outage. SSE resumes from a durable event sequence and uses the database log, so reconnection does not depend on an in-memory broadcast buffer.

The marketplace is a self-hosted, versioned manifest repository exposed by the node API. Installation validates metadata and dependencies, verifies the digest and atomically registers the entity with local configuration. Payments, rankings, sandbox runtimes, automatic model routing, vector memory, full A2A compatibility and Kubernetes orchestration are outside this version.

## Implementation and verification sequence

1. Define entities, schema validation, PostgreSQL migrations and task transitions.
2. Implement providers, tool adapters, context compaction and the durable worker.
3. Connect event delivery, federation, interaction and marketplace APIs.
4. Build the localized dashboard on those APIs, including task ownership, mesh, execution and human controls.
5. Verify with real PostgreSQL and NATS, scripted provider protocol fixtures, two independent nodes, concurrent claims, redelivery and worker process termination/restart. Live commercial model quality is separate from protocol and recovery testing.
