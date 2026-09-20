# Authorization policy service and scoped execution

The policy service implements policy management, decisions, revocable subject credentials, scoped workspace and interaction APIs, tenant catalog approvals and local execution for FR-AUTH-001. The management endpoints below require the existing operator bearer token. Remote federation, finer resource scopes, remaining management workflows and dashboard policy controls remain under development; this is not completion of FR-AUTH-001.

## Management API

| Method | Path                                                  | Behavior                                                                  |
| ------ | ----------------------------------------------------- | ------------------------------------------------------------------------- |
| POST   | `/api/authorization/{tenant}`                         | Validate and atomically replace a policy bundle using `expected_revision` |
| GET    | `/api/authorization/{tenant}`                         | Read the current bundle and revision                                      |
| POST   | `/api/authorization/{tenant}/evaluate`                | Evaluate against the current revision and persist a decision audit        |
| POST   | `/api/authorization/{tenant}/simulate`                | Evaluate without writing an audit or executing an operation               |
| GET    | `/api/authorization/{tenant}/revisions`               | Read immutable policy history using `after` (revision) and `limit`        |
| GET    | `/api/authorization/{tenant}/decisions`               | Read decision history using `after` (sequence) and `limit`                |
| POST   | `/api/authorization/{tenant}/credentials`             | Issue a subject bearer token, returned once                               |
| GET    | `/api/authorization/{tenant}/credentials`             | List the latest 200 credential metadata records, without tokens or hashes |
| POST   | `/api/authorization/{tenant}/credentials/{id}/revoke` | Idempotently revoke a credential in the named tenant                      |
| GET    | `/api/authorization/{tenant}/catalog`                 | List tenant Registry approvals and revisions                              |
| POST   | `/api/authorization/{tenant}/catalog`                 | Approve or disable an exact Registry version using `expected_revision`    |

Creation requires `expected_revision: 0`. Subsequent changes require the current revision. Concurrent updates with the same revision cannot both succeed; the loser receives HTTP 409. Revisions and their full policy documents are committed together. History pages default to 100 records and are capped at 200.

Example creation body:

```json
{
  "expected_revision": 0,
  "bundle": {
    "tenant": "acme",
    "subjects": {
      "alice": {
        "kind": "user",
        "roles": ["researcher"],
        "attributes": { "team": "research" }
      }
    },
    "roles": { "researcher": {} },
    "policies": [
      {
        "id": "team-workspace-read",
        "effect": "allow",
        "subjects": { "roles": ["researcher"] },
        "actions": ["workspace.read"],
        "resources": { "kinds": ["workspace"] },
        "condition": {
          "op": "eq",
          "left": { "source": "subject", "path": "/team" },
          "right": { "source": "resource", "path": "/team" }
        }
      }
    ]
  }
}
```

Example evaluation body:

```json
{
  "subject": "alice",
  "action": "workspace.read",
  "resource": {
    "tenant": "acme",
    "kind": "workspace",
    "id": "workspace-1",
    "attributes": { "team": "research" }
  },
  "environment": { "network": "vpn" }
}
```

An evaluation is an operator-supplied scenario, not a credential or permission to execute the named action. Execution integrations must resolve resource and environment attributes from trusted state and inspect `Decision.allowed` before operating.

## Subject credentials and workspace access

After creating a policy bundle, the operator can issue a credential with `{"subject":"alice","expires_in_seconds":3600}`. The subject and every delegation ancestor must exist and be enabled. Lifetime defaults to one hour and must be between one second and 30 days. The response contains `credential` metadata and a single `token`; store the token when receiving it. PostgreSQL stores only its SHA-256 digest. Authenticated responses include `Cache-Control: no-store`.

Use `Authorization: Bearer <subject token>` for scoped operations. Neither a request body nor a header can select another subject or tenant. Each protected transaction rechecks credential expiry/revocation and the current policy. A disabled subject or ancestor is denied immediately at the next boundary. Unknown, expired and revoked tokens receive HTTP 401; insufficient authority receives HTTP 403.

Scoped workspace creation saves tenant and owner independently of editable workspace state, in the same transaction as the workspace, event and authorization decision. Resource kind is `workspace`, resource ID is its UUID, and trusted attributes are `owner` (the creating subject ID) and `workspace_id`. Environment attributes are `node_id` and `transport: "api"`. Setting `owner` or `tenant` inside workspace state never changes authority; extra creation fields are rejected.

| Operation                                               | Required action                                                                         |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Create a workspace                                      | `workspace.create`                                                                      |
| Read a workspace snapshot or include it in `/api/state` | `workspace.read`                                                                        |
| Update workspace state                                  | Both `workspace.read` and `workspace.update` (the response includes the full workspace) |
| Create a task in a workspace                            | `task.create`                                                                           |
| Append a workspace message                              | `message.create`                                                                        |
| Read or stream workspace events                         | Both `workspace.read` and `workspace.events`                                            |

For example, an allow policy with `subjects: {"ids":["alice"]}`, these actions, `resources: {"kinds":["workspace"]}` and a condition comparing resource `/owner` to literal `"alice"` grants Alice access to her workspaces in that tenant. Another tenant's Alice cannot access them. Workspace read opens the workspace. Its tasks, artifacts and messages additionally require `task.read`, `artifact.read` and `message.read` on each stored record. Artifact reads also require the producing task to be readable. Existing policies that only grant workspace actions must explicitly add these resource actions; no implicit content grant is retained.

Collection responses filter by persisted tenant/ownership and the active policy. Scoped `/api/state` includes only approved, readable Registry entries and omits peer and installation data. Run details, control responses, state collections, human-request listings and run-related events also require the run's `task.read`, `run.read`, `memory.read`, `human.read` for its human requests, and current read permission for recorded sources. The human-read gate also protects prompts and answers copied into a run journal. These filters apply to API snapshots, SSE frames and the workspace snapshots supplied to models and observation tools. Marketplace, Registry writes, peer administration and authorization management remain operator-only. Preexisting workspaces remain operator-owned; they are never implicitly assigned to the first tenant. The operator token retains privileged bootstrap, legacy operation and recovery access.

SSE validates scope before sending HTTP 200 and reloads credential/policy authority before each event frame, including already buffered events. It holds no database lock while yielding a frame. Polling without delivery does not append repeated decision audits. Credential revocation interrupts the stream; permission revocation prevents further matching delivery and denies reconnection to the revoked workspace.

## Tenant catalog and local execution

An operator must approve every exact agent, model, tool, skill and cluster version needed by a tenant. For example, POST `{"entry":{"id":"research","version":"1.0.0"},"enabled":true,"expected_revision":0}` to `/api/authorization/acme/catalog`. Approval changes use optimistic revisions and retain history in PostgreSQL. Approval alone does not grant a policy action; policy permission alone does not expose an unapproved entry. Scoped Registry reads and discovery return only approved entries satisfying `registry.read`. Scoped discovery currently searches the local node.

Register the executor as an enabled policy subject with kind `agent` and its canonical ID, such as `aidash://node-a/agents/research@1.0.0`. A subject can POST `{"revision":0,"agent":{"id":"research","version":"1.0.0"}}` to `/api/tasks/{id}/claim`, or delegate to the local node through `/api/tasks/{id}/delegate`. The task must be OPEN with completed dependencies. Admission checks workspace access, `agent.execute` and `task.execute`, then commits the task claim, run and execution grant together. Local delegation additionally requires `task.delegate` for the originating chain and records its delivery atomically.

The grant stores tenant, workspace, root subject, credential ID and the complete runtime agent chain; it contains no bearer secret. Every execution decision must allow both the originating subject and every agent in that chain, including each subject's policy-defined delegation ancestors. A child cannot escape a parent's denial. The chain is bounded to 32 subjects, including the root.

| Boundary                   | Required action and resource                                                                                                                       |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Every worker step          | `workspace.read` on the workspace, `task.execute` on the task, `agent.execute` on the agent, `run.read` on the run and `memory.read` on its memory |
| Load configured components | `registry.read` on the approved model/tool/skill/cluster entries                                                                                   |
| Model inference            | `model.infer` on the model, `skill.use` on each skill and `memory.read`                                                                            |
| Invoke a tool              | `tool.invoke` on the configured tool ID or `builtin:<tool name>`                                                                                   |
| Builtin mutations          | `task.create`, `task.delegate`, `artifact.create`, `message.create`, `memory.write` or `human.request` for the corresponding resource              |
| Complete a task            | `artifact.create` and `task.complete`                                                                                                              |
| Control or resume a run    | `workspace.read`, `run.read`, `memory.read` and `run.control`                                                                                      |

Runtime resource attributes include trusted workspace `owner` and `workspace_id`. Tasks add their stored `created_by` and `task_id`; artifacts add their stored `created_by`, `task_id` and content `kind`; messages expose their persisted author as `created_by` and `sender`. Registry resources add their stored version, capabilities, tags, languages and configuration. Memory uses the agent ID with its stored run's `version` and canonical agent `created_by`. JSON payloads cannot replace these authority attributes. Artifact creation is scoped to the producing task ID; task/message creation is scoped to the workspace ID. The environment identifies the node and `transport: "worker"`. These attributes come from persisted state.

Each worker boundary reloads and locks the current policy, credential, grant, workspace ownership and relevant catalog approvals. Its decision audits commit before model/tool effects start. Revocations wait for an in-flight boundary to finish, then prevent the next boundary. A denied or revoked execution pauses while retaining phase, context and pending tool cursor. Restoring permissions and resuming continues the saved operation under a fresh check. A fresh credential for the original root subject can replace an expired/revoked grant credential through resume; the agent chain remains unchanged. Authorized cancellation can clean up after revocation without new model or tool calls.

The `serve` process reserves a separate database pool for workers so API requests waiting to revoke authority cannot exhaust the connections needed to finish an effect. Embedded runners should construct their worker Federation with `Federation::for_workers()` before starting workers. Pool settings and connection initialization hooks are preserved. Deploy this version to all workers before admitting scoped runs; older workers do not enforce execution grants.

Scoped remote delegation and peer admission remain closed pending trusted cross-node identity mapping. Legacy admission also rejects scoped workspaces, including operator requests through that path; use subject credentials for scoped local admission. Tasks created in legacy operator workspaces retain their existing behavior.

## Scoped conversations and human interaction

The dashboard accepts either an operator token or a tenant subject token. `/api/state` returns the authenticated access profile; the dashboard uses it to show tenant/subject identity and avoid administrator-only mesh, Marketplace and registration flows. Authorization remains enforced on the server. Invalid, expired or revoked credentials clear the dashboard's authenticated cache and return to the connection screen. Access-denied and reconnect messages support en-US and ja-JP.

| Operation                                     | Additional required actions                                                                                                             |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Start a conversation with an agent or cluster | `conversation.create` and `conversation.read`, workspace create/read, message/task create, task read/delegate/execute and agent execute |
| Select a cluster coordinator                  | `registry.read` and `cluster.execute` on the approved cluster version                                                                   |
| Send a message to a run                       | `run.message` and `message.create`, plus the run's read gates                                                                           |
| Answer a human request                        | `human.read` and `human.answer`, plus the run's read gates                                                                              |
| Abandon a failed, blocked or cancelled task   | `workspace.read`, `task.read` and `task.abandon`                                                                                        |

Conversation admission atomically creates workspace ownership, conversation, initial message, task, local delegation and execution grant. A late policy denial rolls back all these changes while retaining its authorization decisions. Conversation and message authors come from the authenticated subject. The returned task reflects its committed claim. Cluster approvals are rechecked at every worker boundary, even when the agent configuration does not repeat the conversation's cluster reference.

Conversation reads and events require `conversation.read`; trusted attributes include `created_by`, `target`, `target_kind`, workspace ID and owner. Human-request reads and events require `human.read`; their attributes include request `kind`, `run_id`, workspace ID and owner. A worker cannot consume a denied human response. The human-request membership of a run is rechecked during event replay so a newly created denied request cannot bypass a cached run decision.

Human answers persist `answered_by`. Repeating the same answer preserves the original actor and produces no duplicate answer event; conflicting answers return HTTP 409. Task abandonment retains the existing revision/child-state constraints and records the authenticated actor in its event. Cross-tenant interaction IDs return HTTP 403 before exposing resource state.

## Resource reads and journal sources

Workspace snapshots, `/api/state`, event replay, every SSE frame, model context and observation tools filter individual tasks, artifacts and messages using the same stored-resource resolvers. Run visibility requires its task to be readable. Creation and assignment responses also enforce task read permission: unreadable new-task responses roll back their writes while retaining the denial audit, and unreadable parents/dependencies cannot be used to create a new task. A claim cannot return or execute a task that the caller cannot read.

New message events include the immutable message ID. Older events lacking IDs require read permission for every matching stored message; ambiguous identity never bypasses a denied record. Unknown event families are withheld until an explicit scoped reader exists. A completion event containing both task and artifact requires both reads.

A worker commits the IDs of records in an authorized workspace snapshot to `authorization_run_reads` before a provider or tool can copy them into a durable journal. Its created tasks, artifacts, messages and generation assignments are recorded in the same transaction as their output, including idempotent replay. API reads, controls, event replay and worker boundaries recheck those sources against current policy. Revoking a source or output hides an affected journal and pauses its execution before another model/tool effect, even if a previously authorized snapshot is already in history. Sources withheld from a new snapshot are not recorded and do not block otherwise authorized work. Dependencies between observed runs are traversed iteratively, including cycles; source membership and human-request membership are rechecked rather than assuming cached membership is complete.

The migration conservatively associates older nonempty journals with their existing workspace records because they did not record exact source membership. Those older journals may therefore need broader read grants than newly recorded snapshots. This protects retained data instead of treating missing provenance as permission to disclose it. Independently authorized output records retain their own read policies; this ledger does not claim arbitrary information-flow tracking for generated prose or complete memory-source lifecycle enforcement. Semantic-memory authorization remains in the expanded implementation plan.

## Policy semantics

Subjects have kind `user`, `agent`, `node` or `service`, direct roles, groups, attributes, an `enabled` flag and an optional `delegated_by` subject. Groups grant roles; roles inherit other roles. Subject selectors combine IDs, kinds, groups and effective roles with OR. Use `{"any": true}` alone to match any registered, enabled subject in the tenant. Actions and resource kinds must be nonempty sets. Resource IDs optionally narrow the scope. Names are case-sensitive; only a complete `"*"` is a wildcard in action/resource selectors.

Policies default to deny, and any matching explicit deny overrides all allows regardless of order. A resource in another tenant always denies. Delegated subjects must also pass the same action/resource evaluation for every ancestor, using that ancestor's current attributes and authority. Disabling a delegator revokes its descendants' effective authority on the next evaluation.

Conditions support `all`, `any`, `eq`, `not_eq`, array `contains` and `exists`. Operands read JSON pointers from subject/resource/environment attributes or contain a literal JSON value. Missing operands fail comparisons, including `not_eq`; explicit JSON null remains a value. Empty condition groups, unknown fields, dangling references, duplicate policy IDs, role/delegation cycles and excessive expression depth are rejected. Bundles allow at most 512 subjects, groups, roles and policies each; role/delegation chains are limited to 32, and each expression to depth 16 and 256 nodes.

Decisions include the revision, reason, matched policy IDs and the subject's effective roles. Audits retain identifiers and the decision, omitting raw subject/resource/environment attributes. Dry-run requests produce no writes.

## Transaction boundary

`Authorization::evaluate_in_transaction` holds a shared lock on the policy revision until the caller commits or rolls back. A mutation integration must execute its protected change in that same transaction; calling `evaluate`, then starting a separate mutation transaction would allow revocation to race the operation. Scoped compound operations hold the policy/credential locks before a mutation savepoint and defer their decision audit until finalization; a policy denial rolls back protected writes before committing the audit. Long-running work must recheck at each persisted execution or delivery boundary.

PostgreSQL integration tests verify API authentication, revision conflicts, decision/history persistence, dry-run behavior, revoked delegation, two-tenant workspace isolation, ownership forgery rejection, credential expiry/revocation, buffered SSE revocation and policy/credential locks held through a real workspace mutation. Local worker tests cover approved catalog isolation, disabled-agent and tool-policy denial, pending-operation recovery, child authority intersection, credential rotation, cancellation after revocation, run visibility in control/collection/stream/tool paths, durable audit before an HTTP effect and revocation with all API pool connections occupied. Pure policy tests cover inherited group roles, attribute conditions, deny precedence, tenant isolation, missing attributes and invalid policy graphs. These tests do not establish mesh-wide enforcement or completion of FR-AUTH-001.

Approved generation compaction uses resource kind `compactor` and action `compaction.invoke`, in addition to `registry.read`. The worker intersects all originating subjects and retains the live policy/credential/catalog lease through each bounded provider call. See [the compaction contract](generation.md#approved-compaction).
