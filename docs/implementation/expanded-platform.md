# Expanded v0.1.0 implementation plan

**Goal:** Implement and push all six mandatory capabilities in [the expanded requirements](../expanded-requirements.md) to PR #2, with their specified verification. The existing baseline and document-only commit do not satisfy this goal.

**Architecture:** Keep the Rust node and independently owned PostgreSQL databases. Add shared authorization, a durable transaction participant/coordinator, policy-driven agent provisioning, a vector retrieval service, A2A protocol adapters and Kubernetes deployment management. Each capability exposes typed management APIs and dashboard controls; every data path uses the same authority and workspace scope.

**Execution:** Work in the current agent on `feat/v0.1.0-expanded-platform`. Preserve the separate OpenRouter/Jev and review worktrees. Rebase onto the current PR head before pushing. Implement and verify the tasks below continuously; the goal remains active until all six acceptance gates pass.

## Global constraints

- Rust 1.96, edition 2024; axum, PostgreSQL, SQLx/SeaORM, NATS JetStream and the existing React dashboard.
- Explicit model selection and the ordinary durable tool-effect contract remain intact.
- Tenant/workspace authorization applies to API, federation, A2A, tools, search and SSE, including revocation during long-running work.
- Atomic execution requires cooperating participants; reject nonparticipating effects before execution. Compensation and an outbox alone are insufficient.
- A2A compatibility targets v1.0.0, client and server, JSON-RPC, HTTP+JSON and gRPC.
- Verify deployment and recovery on both Kubernetes and k3s. Do not infer this from manifest validation.
- Keep en-US and ja-JP controls and failure messages in sync.

## Task 1: Authorization policy service

Files: `src/authorization/{mod,policy,api}.rs`, `migrations/0004_authorization.sql`, `src/api.rs`, `src/lib.rs`, `tests/authorization.rs`.

Interfaces: `PolicyBundle` holds a tenant's authoritative subjects, groups, role inheritance and policies. `Evaluation` names the subject, action, trusted resource attributes and environment. `PolicyBundle::evaluate(&Evaluation) -> Decision` applies tenant isolation, default deny, explicit-deny precedence and the intersection of each delegation ancestor's authority. `Authorization::replace` uses an expected revision and records the full immutable history. `Authorization::evaluate` reloads the active revision and records the decision; `simulate` does not write.

- [x] Add an HTTP/PostgreSQL regression test: authenticated policy creation must succeed, unauthenticated creation must fail, a role+attribute match allows, a conflicting deny wins, foreign tenant and missing attribute deny, revocation changes the next decision, and stale updates cannot replace current policies. Observe failure against the missing routes.
- [x] Implement strict policy validation, cycle rejection, bounded expression/role/delegation depth, case-sensitive exact identifiers and diagnostic decisions. Unknown attributes fail closed, including inequality.
- [x] Persist revision-checked policy updates and audit history in one SQL transaction; add protected management, evaluation, simulation and history APIs.
- [x] Run `cargo test --locked --test authorization -- --include-ignored`, policy unit tests and affected lint; retain evidence in `.ignore/platform/`.

## Task 2: Enforce authority throughout the existing mesh

Files: `src/authorization/identity.rs`, `src/api.rs`, `src/store.rs`, `src/federation.rs`, `src/harness.rs`, `src/tool.rs`, schema migrations and authorization integration tests.

- [x] Introduce hashed, expiring and revocable subject credentials with persisted workspace ownership. Protect workspace creation, state updates, task/message creation, aggregate reads, state collections and each SSE frame in transactions holding current policy/credential locks.
- [x] Add tenant catalog approvals and atomic local run admission with durable root credential and agent-chain grants. Recheck policy, credentials and catalog approvals at each worker boundary; filter run data in controls, collections, streams and worker observations.
- [x] Add atomic scoped conversations, attributed human answers and run messages, scoped abandonment, human/conversation read gates, and subject-token dashboard operation with revocation handling.
- [ ] Extend persisted ownership and fine-grained authorization to the remaining resource kinds. Finish migrating ordinary workflows to scoped credentials, retaining the operator only for administrative bootstrap/recovery.
- [ ] Carry the authenticated subject and tenant into created workspaces, conversations, tasks, runs, artifacts and memory. Resolve resource attributes from stored state, never caller claims.
- [ ] Authorize reads and mutations in their transaction; filter collection/search results and each SSE frame. Recheck authority at durable execution and tool boundaries.
- [ ] Persist delegated authority, enforce its intersection at both nodes, and map external A2A identity to a local trusted subject.
- [ ] Verify two-tenant isolation for every endpoint, search, artifact, memory and stream, forged ownership, role changes, credential revocation and remote delegation.

## Task 3: Distributed transaction participant and coordinator

Files: `src/transactions/{mod,participant,coordinator,api}.rs`, schema migration, Store/Registry mutation interfaces, federation recovery loop and `tests/transactions.rs`.

- [ ] Define a validated transaction manifest with global ID, exact participant set, expected resource revisions, typed mutations and deadline. Acquire a globally ordered transaction lease before prepare; retain it through finalization/recovery to prevent distributed serialization cycles.
- [ ] Participants persist prepared writes and locks, with serializable validation against authoritative resource revisions. Every ordinary read/write must honor these locks; no bypass through Registry, workers or SSE is permitted.
- [ ] Persist the coordinator's immutable commit/abort decision before issuing finalize messages. Repeated prepare/finalize must verify the same input and decision. An uncertain participant queries durable authority and waits when unreachable.
- [ ] Keep prepared changes invisible until all participants have durably accepted the decision; release visibility barriers only after this point. Publish the corresponding events with the visible updates.
- [ ] Reject unsupported external effects at validation. Expose participant state, recovery, deadlines and wait reasons.
- [ ] Test commit/abort and conflicting concurrent transactions on two independently persisted nodes. Inject failure before/after each durable phase and network partitions, then verify serializable observations, durable agreement and no duplicate effects.

## Task 4: Policy-driven automatic agent generation

Files: `src/generation/{mod,api}.rs`, Registry/Harness integration, schema migration, dashboard forms and integration/browser tests.

- [x] Persist generation policies containing approved template, explicit model, tool/skill allowlists, permissions, count/concurrency/depth/budget/lifetime limits and approval requirement.
- [x] On an unsatisfied task, reserve quota and idempotency key atomically, construct and schema-validate the agent, intersect originating authority, register it, optionally request approval, then schedule it.
- [x] Persist all provisioning stages and resume after process failure. Record origin, policy/version, reason and lifecycle history; expire and stop agents without losing task journals.
- [ ] Complete the full generation acceptance matrix, including process-level provisioning restart, all limits and denied tools; add bilingual controls and audit views. Backend PostgreSQL tests cover missing-agent completion, deduplication, concurrent quota, denied approval, nested authority/depth, credential revocation, token exhaustion, expiry and stop during inference.
- [ ] Add an explicit approved and budgeted compaction provider contract for generated definitions; currently a context requiring external Jev compaction fails before disclosure.

## Task 5: Semantic memory with a vector database

Files: `src/memory/{mod,embedding,index,api}.rs`, schema migration, Harness/Tool integration, deployment configuration and tests.

- [ ] Use a configurable embedding provider and persistent vector backend; store source/version, embedding model/version and tenant/workspace/agent scope separately from vectors.
- [ ] Journal ingestion, updates, deletions, reindex and embedding-model migration with stable keys. Preserve tombstones and authorization filters so stale indexes cannot return deleted/revoked data.
- [ ] Authorize before retrieval and recheck each result against the live source, preserve provenance, and fit selected results within the context budget. Expose pending/failed indexing and explicit backend failure/degraded state.
- [ ] Verify semantic relevance without a keyword match using a fixed documented fixture, provider protocol behavior, real vector persistence/restart, all lifecycle operations and authorization isolation. Add bilingual management controls.

## Task 6: A2A v1.0.0 interoperability

Files: `src/a2a/{mod,card,service,client,jsonrpc,http,grpc,push}.rs`, pinned protocol definitions, schema migration, dashboard and interoperability tests.

- [ ] Pin and generate official v1.0.0 types. Implement one shared operation service behind all three protocol bindings and a matching outbound client.
- [ ] Persist mappings for context/message/task/artifact IDs and status, including continuation, cancellation, streaming subscription, push configuration/delivery and extended Agent Cards.
- [ ] Apply authentication and the shared authorization service; validate protocol versions, errors and advertised capabilities. Use persisted event cursors for reconnect/restart continuity.
- [ ] Run the normative conformance matrix plus bidirectional tests against an independently implemented peer for every binding, reconnect, push, cancellation, authentication rejection and version mismatch. Record source version and evidence before asserting full compatibility.

## Task 7: Kubernetes and k3s orchestration

Files: `Dockerfile`, `deploy/`, `src/orchestration/{mod,api}.rs`, graceful shutdown in `src/main.rs`, dashboard and cluster acceptance scripts.

- [ ] Supply versioned declarative deployment configuration for independent node servers/workers, stable identity, external PostgreSQL/NATS connections, persistent state, Secret references, probes, resource limits and graceful termination.
- [ ] Add authorized cluster/deployment management for apply/update/stop/scale and read status/replicas/failures. Use Kubernetes API credentials only through configured references.
- [ ] Run the two-node golden path on Kubernetes and k3s separately; terminate worker pods, scale workers and roll versions while checking original task/run recovery, artifacts and effect counts.

## Task 8: Integrated acceptance, review and push

- [ ] Run the expanded golden path on both cluster types: authorized goal, missing-agent generation, semantic context, independent A2A collaboration, atomic state finalization, pod failure and network partition/recovery.
- [ ] Verify all required rejection cases and bilingual dashboard controls. Run the complete repository checks on the final integrated head.
- [ ] Review every requirement against concrete evidence, update the requirement map and Notion implementation status, recheck destination contribution policy, and push to PR #2 without force.
- [ ] Prove local HEAD, upstream, `git ls-remote` and PR `headRefOid` agree. Report remote CI separately from local validation. Mark the goal complete only when every requirement has passed its acceptance gate.

## Review focus

Missing attributes and malformed policies must never grant access; cycles must terminate. Revocation must affect an already running operation or stream at its next boundary. Transaction uncertainty must never release partial visibility. Retried generation and indexing must not bypass quotas or resurrect deleted sources. A2A capability declarations and orchestration status must reflect the actually running implementation.

## Execution evidence

- Task 1: HTTP and OpenAPI regression tests demonstrated the missing policy routes before implementation. After integration with PR head `05684a1`, all 39 Rust tests passed, including ten PostgreSQL tests, with the database and peer fixtures from `scripts/check.sh`.
- OpenAPI export and Orval client generation succeeded; all 59 local schema references resolve. The frontend production build passed. Clippy passed with warnings denied, and Trunk checked 147 files with no findings (Clippy was run separately).
- Policy schema recursion is represented by references rather than recursively collecting the same schema. The contract test verifies the reference and all six authenticated operations.
- Task 2, workspace boundary: credential issuance first failed with HTTP 405, then passed with tenant-scoped identity and persisted ownership. Tests cover two tenants with the same subject name, forged ownership, forbidden collection/event data, live policy changes, credential expiry/revocation, stream revocation after buffering and policy/credential locks held through an actual workspace update. An explicitly denied read cannot be bypassed through an update response; the rejection audit persists without the attempted mutation.
- All 44 Rust tests passed, including fifteen PostgreSQL integration tests. OpenAPI export and Orval generation succeeded with 35 operations and 115 resolving schema references. The frontend production build, Clippy with warnings denied and Trunk on 152 files passed.
- The existing two-node golden path passed through the dashboard, including all four browser cases. Four tasks and artifacts completed with three external effects; the killed remote worker recovered its original run, and 175 queued events survived the NATS outage. This is baseline regression evidence, not the pending expanded cluster/A2A/transaction acceptance gate.
- Task 2, local execution: catalog admission first failed with HTTP 405. The implemented path persists tenant approvals, root credentials and child-agent chains, then enforces their intersection at every worker boundary. Five PostgreSQL tests verify denied admission without partial state, tenant isolation, disabled agents, tool denials, pending-operation resume, child delegation, credential rotation, cancellation and run visibility across direct/control/collection/event/tool paths.
- A blocked HTTP fixture verifies that root and agent decision audits are durable before the effect returns. Twelve waiting policy/credential revocations exhaust the API pool while the isolated worker pool still saves the result and releases its authority lease. The next boundary pauses with its tool cursor intact.
- All 49 Rust tests passed, including twenty PostgreSQL tests. OpenAPI export and Orval generation succeeded with 37 operations and 119 resolving schema references. The frontend build, Clippy with warnings denied and Trunk on 158 files passed.
- The two-node dashboard golden path passed again after local worker integration: all four browser cases, four tasks/artifacts, three external effects, original-run recovery after SIGKILL, 175 queued events during the NATS outage and 222 observed SSE events. Evidence is retained in `.ignore/platform/execution-auth-*` and `.ignore/acceptance/report.json`. This remains baseline regression evidence, not completion of the expanded acceptance gates.
- Task 2, scoped interaction: an HTTP regression first observed conversation admission being rejected by the operator-only route. Three PostgreSQL tests now verify atomic admission and rejection audit without orphan rows, persisted authors/answer actors, duplicate/conflicting answers, cross-tenant and action denials, human/conversation read filtering including run journals, worker response denial, task abandonment and cluster approval revocation.
- All 52 Rust tests passed, including twenty-three PostgreSQL tests. OpenAPI and generated clients validate 37 operations and 120 schema references. Clippy, the frontend build and Trunk on 163 files passed. The two-node dashboard acceptance passed with four tasks/artifacts, three effects, worker SIGKILL recovery, 169 queued NATS-outage events and 216 observed SSE events.
- Five browser cases passed. The new case uses a real scoped credential and a local model fixture to start a conversation, answer a human request and complete its task/artifact, then verifies tenant identity, bilingual controls, absence of administrator API calls and clearing of the session/cache after revocation. The scoped settings screenshot was visually inspected. The acceptance report now derives its browser count from Playwright output. Evidence: `.ignore/platform/interaction-auth-*`, `.ignore/acceptance/report.json` and `.ignore/dashboard-scoped-access.png`.
- Tasks 2–8 remain open. Scoped tasks support local admission and local child delegation with intersected authority. Scoped remote delegation and legacy admission remain closed. Finer resource-level grants, the remaining mesh paths and dashboard policy controls still require implementation; none of the six expanded requirements is complete end to end.

- Task 4, generation backend: twelve PostgreSQL tests verify pinned policies, atomic quotas, approval/denial, immutable registration and scoped admission, missing-agent completion, existing-agent reuse, nested worker generation with inherited authority/depth, revoked credentials, missing usage, expiry, read/event isolation and stop racing an in-flight model call. Provider tests also reject incomplete usage refunds and include Anthropic cache counters.
- All 65 Rust tests passed, including thirty-five PostgreSQL integration tests. OpenAPI and generated clients validate 43 operations and 138 schema references, including separate authorization and generation policy schemas. Clippy with warnings denied, the production frontend build and Trunk on 183 files passed.
- The two-node dashboard acceptance passed all five browser cases: four tasks/artifacts, three external effects, original-run recovery after SIGKILL, 169 queued NATS-outage events and 206 observed SSE events. Evidence is in `.ignore/platform/generation-*` and `.ignore/acceptance/report.json`. Generation dashboard controls, approved compaction and the full expanded release acceptance remain pending.
