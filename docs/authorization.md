# Authorization policy service

The policy service implements the policy management and decision portion of FR-AUTH-001. All endpoints below require the existing operator bearer token. Mesh-wide enforcement, subject credentials and dashboard controls are tracked in [the implementation plan](implementation/expanded-platform.md); registering a subject here does not grant that subject access to the existing mesh APIs.

## Management API

| Method | Path                                    | Behavior                                                                  |
| ------ | --------------------------------------- | ------------------------------------------------------------------------- |
| POST   | `/api/authorization/{tenant}`           | Validate and atomically replace a policy bundle using `expected_revision` |
| GET    | `/api/authorization/{tenant}`           | Read the current bundle and revision                                      |
| POST   | `/api/authorization/{tenant}/evaluate`  | Evaluate against the current revision and persist a decision audit        |
| POST   | `/api/authorization/{tenant}/simulate`  | Evaluate without writing an audit or executing an operation               |
| GET    | `/api/authorization/{tenant}/revisions` | Read immutable policy history using `after` (revision) and `limit`        |
| GET    | `/api/authorization/{tenant}/decisions` | Read decision history using `after` (sequence) and `limit`                |

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

## Policy semantics

Subjects have kind `user`, `agent`, `node` or `service`, direct roles, groups, attributes, an `enabled` flag and an optional `delegated_by` subject. Groups grant roles; roles inherit other roles. Subject selectors combine IDs, kinds, groups and effective roles with OR. Use `{"any": true}` alone to match any registered, enabled subject in the tenant. Actions and resource kinds must be nonempty sets. Resource IDs optionally narrow the scope. Names are case-sensitive; only a complete `"*"` is a wildcard in action/resource selectors.

Policies default to deny, and any matching explicit deny overrides all allows regardless of order. A resource in another tenant always denies. Delegated subjects must also pass the same action/resource evaluation for every ancestor, using that ancestor's current attributes and authority. Disabling a delegator revokes its descendants' effective authority on the next evaluation.

Conditions support `all`, `any`, `eq`, `not_eq`, array `contains` and `exists`. Operands read JSON pointers from subject/resource/environment attributes or contain a literal JSON value. Missing operands fail comparisons, including `not_eq`; explicit JSON null remains a value. Empty condition groups, unknown fields, dangling references, duplicate policy IDs, role/delegation cycles and excessive expression depth are rejected. Bundles allow at most 512 subjects, groups, roles and policies each; role/delegation chains are limited to 32, and each expression to depth 16 and 256 nodes.

Decisions include the revision, reason, matched policy IDs and the subject's effective roles. Audits retain identifiers and the decision, omitting raw subject/resource/environment attributes. Dry-run requests produce no writes.

## Transaction boundary

`Authorization::evaluate_in_transaction` holds a shared lock on the policy revision until the caller commits or rolls back. A mutation integration must execute its protected change in that same transaction; calling `evaluate`, then starting a separate mutation transaction would allow revocation to race the operation. Long-running work must recheck at each persisted execution or delivery boundary.

PostgreSQL integration tests verify API authentication, revision conflicts, decision/history persistence, dry-run behavior, revoked delegation, policy-lock serialization and concurrent updates. Pure policy tests cover inherited group roles, attribute conditions, deny precedence, tenant isolation, missing attributes and invalid policy graphs. These tests do not establish mesh-wide enforcement or completion of FR-AUTH-001.
