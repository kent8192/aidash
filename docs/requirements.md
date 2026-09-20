# v0.1.0 requirement map

The source of scope is the [Notion functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24), including the six mandatory additions in section 11 (last edited 2026-09-20 15:55:40 UTC). The table below maps the existing implementation. Additional requirements and their pending implementation status follow separately; passing the original acceptance scenario is no longer sufficient for a v0.1.0 release.

| Requirement                    | Implementation                                                                                | Verification                                                |
| ------------------------------ | --------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| FR-HARNESS-001 runtime         | `harness.rs`, persisted identity/model/instructions/context/tools/memory/workspace/event loop | Two-node golden path                                        |
| FR-HARNESS-002 providers       | `provider.rs`, shared trait and OpenAI/Anthropic/OpenRouter adapters                          | Adapter unit tests and HTTP protocol fixtures               |
| FR-HARNESS-003 model selection | Versioned model Registry and explicit agent model reference                                   | Registry validation and agent creation UI                   |
| FR-HARNESS-004 tools           | `tool.rs`, Native/HTTP/MCP/Agent implementations of one trait                                 | Native/HTTP/MCP/Agent protocol acceptance                   |
| FR-HARNESS-005 context         | `context.rs`, Rust fast-jev-compaction, Jev probabilities and pinned events                   | Decision, retention, state fitting, batching and HTTP tests |
| FR-FED-001 identity            | Public discovery document                                                                     | Two independently configured nodes                          |
| FR-FED-002 peers               | Authenticated, symmetric peer registration                                                    | Golden path and unauthorized API tests                      |
| FR-FED-003 discovery           | Conjunctive capability/skill/tag/language/model metadata search                               | Japanese research discovery across two nodes                |
| FR-FED-004 remote task         | Durable offers, scoped grants, versioned HTTP protocol                                        | Remote claim, output, effect and completion                 |
| FR-WORK-001 workspace          | Goal/tasks/events/artifacts/messages/state                                                    | Golden path and browser workspace creation                  |
| FR-WORK-002 tasks              | Fields, state transitions, dependencies, parent and revision                                  | PostgreSQL transition tests                                 |
| FR-WORK-003 claim              | Revision-checked SQL claim                                                                    | Concurrent claim and dependency tests                       |
| FR-WORK-004 artifact           | Text/JSON/file reference/code/structured result                                               | Atomic final artifact and replay tests                      |
| FR-WORK-005 goal               | Goal-to-task-to-subtask hierarchy                                                             | Coordinator decomposes Axum/Actix/Rocket                    |
| FR-EXEC-001 state machine      | PostgreSQL run journal, leases and persisted tool cursor                                      | Lease-fencing integration tests                             |
| FR-EXEC-002 recovery           | Durable journals, JetStream/outbox/inbox and restart scanning                                 | Actual worker SIGKILL and restart                           |
| FR-EXEC-003 idempotency        | Invocation and completion keys, uncertainty reconciliation                                    | Replayed request produces one effect; completion replay     |
| FR-REG-001 entities            | Agent/model/tool/skill/cluster/node Registry                                                  | API and metadata validation                                 |
| FR-REG-002 metadata            | JSON Schema and typed component configuration                                                 | Invalid metadata tests                                      |
| FR-REG-003 versioning          | Immutable semantic versions                                                                   | Conflict and installation tests                             |
| FR-REG-004 search              | Conjunctive metadata filters                                                                  | Local and remote search tests                               |
| FR-MKT-001 browse              | Marketplace catalog/search                                                                    | Browser marketplace flow                                    |
| FR-MKT-002 package page        | Author/version/capabilities/permissions/languages/dependencies                                | Browser package detail                                      |
| FR-MKT-003 install             | Digest check, Registry registration, local configuration                                      | PostgreSQL atomic installation and browser flow             |
| FR-MKT-004 publish             | Self-hosted immutable manifest repository                                                     | Browser publish/install and conflict tests                  |
| FR-INT-001 conversation        | Agent or cluster target linked to a new workspace and goal                                    | Cluster-target golden path                                  |
| FR-INT-002 streaming           | Authenticated SSE with durable replay cursor                                                  | Golden path SSE and browser live updates                    |
| FR-INT-003 human input         | Four durable request types and stored responses                                               | All four request types; remote approval round trip          |
| FR-INT-004 controls            | Pause/resume/cancel/message                                                                   | Pause/resume/message acceptance; cancellation test          |
| FR-UI-001 mesh view            | Node/agent topology, membership and task assignment                                           | Browser navigation and rendered overview                    |
| FR-UI-002 execution view       | Status/task/model context/tool calls/events/errors                                            | Actual run details and event data                           |
| FR-UI-003 workspace view       | Task board with ownership and visible failures                                                | Browser workspace/task flow                                 |
| FR-UI-004 event stream         | Live event view and structured payloads                                                       | SSE sequence and browser tests                              |
| i18n                           | UI catalogs, localized metadata, agent languages                                              | Language switch and localized package test                  |

The acceptance environment uses deterministic protocol fixtures. Passing its tests does not claim live commercial-model quality, provider account access, hostile multi-tenant isolation, or large-scale performance. Those are separate deployment and evaluation activities.

## Additional mandatory requirements

The [expanded requirements](expanded-requirements.md) define the behavior, acceptance criteria, implementation order and remaining exclusions synchronized with Notion. None of the following is deferred to a future version or covered by the existing acceptance evidence.

| Requirement  | Required capability               | Implementation status                                                                                                                                                                    | Required verification                                                                                                                                |
| ------------ | --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| FR-OPS-001   | Kubernetes / k3s orchestration    | Pending                                                                                                                                                                                  | Two-node golden path on each platform; pod termination, scaling and rolling updates                                                                  |
| FR-AGENT-001 | Automatic agent generation        | [Policy, provisioning, inherited authority, limits, lifecycle, approved compaction and bilingual dashboard](generation.md#acceptance-evidence) verified; integrated release gate pending | Missing-agent task completion; retry/restart deduplication, limits and denied approval                                                               |
| FR-AUTH-001  | Complex RBAC / ABAC               | Policy service, workspace/interaction APIs, tenant catalog, local workers and subject dashboard implemented; finer resource scopes, remote paths and policy UI pending                   | Policy/API/PostgreSQL tests, two-tenant isolation, live revocation, local effects and child authority; all-path authorization remains to be verified |
| FR-TX-001    | Complete distributed transactions | Pending                                                                                                                                                                                  | Multi-node commit/abort, phase failures, partition recovery and atomic visibility                                                                    |
| FR-MEM-001   | Semantic memory / vector DB       | Pending                                                                                                                                                                                  | Semantic retrieval, provenance, persistence, deletion/reindexing and permission isolation                                                            |
| FR-A2A-001   | Full A2A compatibility (v1.0.0)   | Pending                                                                                                                                                                                  | Bidirectional independent interoperability, all three bindings and conformance matrix                                                                |

## Existing baseline acceptance evidence

The [authorization policy service](authorization.md) adds revisioned policy management, audited RBAC/ABAC decisions, dry-run evaluation, tenant catalog approvals and scoped workspace/local worker access with revocable credentials. Its focused evidence does not complete FR-AUTH-001 or the expanded release gate.

The two-node test begins with a goal submitted through the dashboard. The coordinator discovers agents, creates Axum/Actix/Rocket subtasks, and delegates across node boundaries. A remote worker is killed after an HTTP effect, then recovers its original run and replays the original invocation key. All four tasks and artifacts complete with three external effects. The same scenario verifies a declined remote approval with pause/message/resume and Native, MCP and Agent tool calls. Separate browser cases cover all eleven screens, workspace/task assignment, locale switching and marketplace publication/installation.

Run `scripts/check.sh` for the full repeatable check, and `trunk check --all --no-fix` for lint. Generated evidence is in `.ignore/acceptance/report.json`; dashboard screenshots are generated locally and retained as CI artifacts.
