# Authorization policy service and workspace access

The policy service implements policy management, decisions, revocable subject credentials and scoped workspace API access for FR-AUTH-001. The management endpoints below require the existing operator bearer token. Execution, federation, Registry/Marketplace resource scopes and dashboard controls are still tracked in [the implementation plan](implementation/expanded-platform.md); this is not completion of FR-AUTH-001.

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

For example, an allow policy with `subjects: {"ids":["alice"]}`, these actions, `resources: {"kinds":["workspace"]}` and a condition comparing resource `/owner` to literal `"alice"` grants Alice access to her workspaces in that tenant. Another tenant's Alice cannot access them. Workspace read currently covers its aggregate contents (tasks, artifacts, messages and related state records); finer per-resource grants remain part of the pending mesh integration.

Collection responses filter by persisted tenant/ownership and the active policy. Scoped `/api/state` omits global Registry, peer and installation data. Global search, Registry, Marketplace, mesh, execution controls and authorization management remain operator-only. Preexisting workspaces remain operator-owned; they are never implicitly assigned to the first tenant. The operator token retains privileged bootstrap, legacy operation and recovery access.

SSE validates scope before sending HTTP 200 and reloads credential/policy authority before each event frame, including already buffered events. It holds no database lock while yielding a frame. Polling without delivery does not append repeated decision audits. Credential revocation interrupts the stream; permission revocation prevents further matching delivery and denies reconnection to the revoked workspace.

Scoped tasks cannot yet be claimed, delegated or accepted into a worker. These boundaries reject scoped workspaces even when requested with the operator token, until durable execution authority is connected. This prevents the existing worker/federation paths from bypassing scoped policy checks. Tasks created in legacy operator workspaces retain their existing behavior.

## Policy semantics

Subjects have kind `user`, `agent`, `node` or `service`, direct roles, groups, attributes, an `enabled` flag and an optional `delegated_by` subject. Groups grant roles; roles inherit other roles. Subject selectors combine IDs, kinds, groups and effective roles with OR. Use `{"any": true}` alone to match any registered, enabled subject in the tenant. Actions and resource kinds must be nonempty sets. Resource IDs optionally narrow the scope. Names are case-sensitive; only a complete `"*"` is a wildcard in action/resource selectors.

Policies default to deny, and any matching explicit deny overrides all allows regardless of order. A resource in another tenant always denies. Delegated subjects must also pass the same action/resource evaluation for every ancestor, using that ancestor's current attributes and authority. Disabling a delegator revokes its descendants' effective authority on the next evaluation.

Conditions support `all`, `any`, `eq`, `not_eq`, array `contains` and `exists`. Operands read JSON pointers from subject/resource/environment attributes or contain a literal JSON value. Missing operands fail comparisons, including `not_eq`; explicit JSON null remains a value. Empty condition groups, unknown fields, dangling references, duplicate policy IDs, role/delegation cycles and excessive expression depth are rejected. Bundles allow at most 512 subjects, groups, roles and policies each; role/delegation chains are limited to 32, and each expression to depth 16 and 256 nodes.

Decisions include the revision, reason, matched policy IDs and the subject's effective roles. Audits retain identifiers and the decision, omitting raw subject/resource/environment attributes. Dry-run requests produce no writes.

## Transaction boundary

`Authorization::evaluate_in_transaction` holds a shared lock on the policy revision until the caller commits or rolls back. A mutation integration must execute its protected change in that same transaction; calling `evaluate`, then starting a separate mutation transaction would allow revocation to race the operation. Long-running work must recheck at each persisted execution or delivery boundary.

PostgreSQL integration tests verify API authentication, revision conflicts, decision/history persistence, dry-run behavior, revoked delegation, two-tenant workspace isolation, ownership forgery rejection, credential expiry/revocation, buffered SSE revocation and policy/credential locks held through a real workspace mutation. Pure policy tests cover inherited group roles, attribute conditions, deny precedence, tenant isolation, missing attributes and invalid policy graphs. These tests do not establish mesh-wide enforcement or completion of FR-AUTH-001.
