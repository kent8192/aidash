# Additional v0.1.0 requirements

Source: [Notion functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24), section 11, last edited 2026-09-20 15:55:40 UTC (2026-09-21 JST).

The following six capabilities are required for v0.1.0. They are not deferred to a later release. This document records the required behavior and acceptance criteria; their implementation and verification remain pending. The existing two-node golden path alone no longer establishes release readiness.

## FR-OPS-001 Kubernetes / k3s orchestration

Deploy, update, stop and scale Aidash nodes, servers and workers on both Kubernetes and k3s. Provide declarative deployment configuration, persistent storage, Secret references, health/readiness probes, resource limits and graceful shutdown, together with PostgreSQL and NATS JetStream connection and operating instructions.

Pod replacement, worker scaling and rolling updates must preserve node identity, tasks, run journals and artifacts. Leases and fencing must prevent duplicate execution. The dashboard must expose deployment state, replica counts, failures and recovery.

**Acceptance:** Run the two-node golden path separately on Kubernetes and k3s. Kill worker pods, scale workers and perform rolling updates. Verify recovery of the original task/run, retained artifacts and no duplicated external effects.

## FR-AGENT-001 Automatic agent generation

When discovery cannot satisfy a goal or task's requirements, generate an agent definition from permitted templates, models, tools and skills; validate it, register it, start it and assign the task.

The creator explicitly selects the model in the generation policy. Automatic model routing remains excluded. Enforce limits on generated agents, concurrency, generation depth, budget, lifetime and permissions. Deduplicate repeated requests and record the source goal/task, policy, definition version, generation reason and stop/deletion history.

The dashboard must let users enable or disable generation, manage its policy and inspect results and failures. Obtain human approval before activation when required by that policy.

**Acceptance:** Complete a task that initially has no suitable agent. Retries and restarts must not create duplicate agents. Reject unauthorized tools and requests above generation or budget limits; a declined approval must prevent activation.

## FR-AUTH-001 Complex RBAC / ABAC

Combine roles, role inheritance and group membership with policies over subject, resource, action and environment attributes. Subjects include users, agents, nodes and service identities. Scope authorization to tenants, workspaces, clusters, agents, tasks, artifacts, memory, Registry and Marketplace resources.

Use default deny, explicit-deny precedence and least privilege. Enforce the same policy across management APIs, federation, A2A, tools, search and SSE. Delegation and generated agents must not increase the originating authority. Provide policy management, versions, change audits, decision explanations, dry runs and revocation. Long-running executions and streams must honor revocation before subsequent operations or delivery.

The dashboard must support managing roles, attribute conditions and resource scopes, and inspecting authorization decisions.

**Acceptance:** Exercise multiple tenants/workspaces, inherited roles, attribute conditions, conflicting allow/deny rules, revocation and remote delegation. Verify both allowed and rejected operations and the absence of unauthorized data in search results, artifacts, memory and SSE.

## FR-TX-001 Complete distributed transactions

Atomically commit or abort changes to Registry, workspace, task, execution state and artifact resources across participating nodes. Identify the participant set, global transaction ID, isolation level, deadline and coordinator/participant states. Require serializable isolation, durable decisions and idempotent retries.

Persist prepare, commit, abort and recovery records. Coordinator or participant failure, network partitions, delayed messages and duplicates must converge on the same decision. Readers must not observe unresolved partial changes. When the decision is unavailable, wait for recovery instead of reporting success. The dashboard must show participants, progress, reasons for waiting and recovery results.

Outbox/inbox delivery, retry deduplication and compensation alone do not satisfy this requirement. An external tool or A2A peer effect can join an atomic transaction only through an adapter that participates in prepare/commit/abort. Reject an atomic request containing a nonparticipating resource before execution. Ordinary tool calls retain the existing durable execution contract.

**Acceptance:** Verify commit and abort across at least two nodes. Stop coordinators and participants in each phase, partition and restore the network, and replay requests. All participants must agree on the decision, with no partial visibility, dirty reads or duplicate finalization.

## FR-MEM-001 Semantic memory / vector DB

Embed memory and authorized messages/artifacts into a persistent vector database. Retrieve related information using semantic similarity and metadata filters, and include the results in agent context.

Track the original data, provenance, tenant/workspace/agent scope and embedding model/version. Make ingestion, updates, deletion, reindexing and model migration idempotent. Exclude revoked or deleted information even when an index is stale, and remove its indexed representation.

Enforce authorization before and after retrieval, result limits and context budgets. Expose index state, failures and retries. A vector database outage must produce a visible failure or explicitly identified degraded state. Deployment configuration must identify the chosen backend and embedding provider.

**Acceptance:** Retrieve related information without an exact keyword match and include its provenance in context. Verify search after restart, updates, deletion, reindexing, embedding-model changes, authorization boundaries and backend failures.

## FR-A2A-001 Full A2A compatibility

Use [A2A Protocol v1.0.0](https://a2a-protocol.org/v1.0.0/specification/) as the compatibility baseline. Support both client and server roles, Agent Cards and discovery, authentication/authorization, messages, tasks and artifacts, task retrieval/listing/cancellation, streaming/resubscription, push notifications, authenticated extended Agent Cards, version negotiation and standard errors.

Implement JSON-RPC, HTTP+JSON and gRPC bindings with equivalent task state and results. Maintain a conformance matrix for mandatory requirements and supported optional capabilities; never advertise unsupported capabilities or extensions. Persist mappings between A2A identifiers/states and Aidash workspaces/tasks/runs across reconnects and restarts.

Define coexistence and translation with the Aidash Federation Protocol. External A2A agents must not require Aidash-specific endpoints or NATS. Show peer compatibility versions, capabilities and connection failures in the dashboard.

**Acceptance:** Test bidirectional interoperability with an independent A2A implementation over every binding. In addition to specification conformance, verify stream reconnection, push notifications, cancellation, rejected authentication, version mismatch and task continuity after restart. Publish the target version, capability matrix and test evidence with any compatibility claim.

## Release acceptance and implementation order

All six requirements and the existing golden path are release gates. In the integrated scenario, two nodes on Kubernetes and k3s receive an authorized goal, generate a missing agent, retrieve semantic memory into context and collaborate with an external A2A agent. Participating nodes atomically finalize their state changes. Pod termination and network partitions must preserve authorization boundaries, tasks/artifacts and transaction decisions after recovery.

Effects from external A2A agents that cannot participate in transactions use the ordinary durable execution contract. Explicitly requesting atomic execution with such a participant must be rejected before execution. Also exercise denied authorization, generation limits, memory deletion, transaction abort and A2A version mismatch. Dashboard controls and observations for the added capabilities must support en-US and ja-JP.

Implementation order follows Notion section 13:

1. **P0:** Harness, Registry, Workspace/Task, RBAC/ABAC, Event Bus, Interaction Server, then distributed transaction participation, durable decisions and recovery foundations.
2. **P1:** Federation, durable execution, Kubernetes/k3s orchestration, automatic agent generation, semantic memory/vector DB, full A2A compatibility and dashboard observability.
3. **P2:** Marketplace, i18n, plugin UX and integrated acceptance of all six additions.

These priorities order the work; they do not defer mandatory requirements beyond v0.1.0.

## Remaining exclusions

Marketplace payments, agent rankings, reputation, automatic model routing, Meta Agents, WASM sandboxing, tens of thousands of agents, automatic translation, automatic agent optimization and a complex workflow designer remain excluded. Billion-agent scale is not an acceptance target for this release.
