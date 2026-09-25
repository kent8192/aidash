# Creator and Trust workbenches

Creator edits tenant-owned agent drafts on the connected node. **Save draft**
increments a revision; validation, tests, and registration use that exact saved
revision. Concurrent saves return a conflict without discarding the caller's
local edits. **Register in Registry** records an immutable ID and semantic
version, the registration-time release notes, and whether a behavioral test had
completed. It does not publish a Marketplace package, approve a tenant Catalog
binding, or grant workspace execution. An unchanged retry returns the same
version; changed content needs a new semantic version. Versions can be inspected
from Creator even when the tenant Catalog does not expose them for execution.

Creator actions use resource kind `agent_draft`: `agent_draft.create`, `.read`,
`.write`, `.test`, `.register`, `.share`, `.transfer`, and `.archive`. Dependency
checks also require `agent_dependency.read` on `registry_entry`. The owner or an
explicit share is required in addition to policy permission; an edit share does
not itself grant Register. Shares include the draft's private documents and
must be acknowledged again if those documents change. The operator can adopt
an existing Registry agent into a tenant, without changing the immutable
definition or its Catalog bindings.

The default test calls the configured model but resolves every Tool call from
explicit simulated fixtures. Missing fixtures stop the test without invoking a
real Tool. A test session stores its draft revision, mode, conversation, Tool
calls, usage, and expiry. Continuing a completed session carries its conversation
forward; **Reset conversation** starts a new session without that history. The
model, dependency, draft, credential, and test-profile authority is rechecked
before each inference. Ordinary test payloads expire after 30 days by default;
metadata remains. Operators configure per-tenant input/output, step, duration,
concurrency, test-retention, and incident-retention limits with
`PUT /api/workbench/test-limits/{tenant}`.

## Confined real-tool profiles

Only an operator can configure a real-tool connection profile. The current
real-mode executor accepts exact Registry HTTP Tool versions marked `read_only`.
It calls a separate test endpoint, never the Tool's production endpoint, and
uses a separate test credential reference when the production Tool has one.
Each call must include an `action` and `resource` matching the profile's
allowlists and must satisfy the Tool's JSON Schema. HTTPS is required except
for loopback test services. Redirects are not followed. Native, MCP, agent
delegation, and write-capable Tools have no real-mode executor; they require
fixtures, so they cannot affect ordinary workspaces through a test. Profile
changes disable an in-flight session's next effect. A dispatched call is
recorded before network I/O; uncertain outcomes are not retried.

For example, after registering an HTTP Tool `lookup@1.0.0` with a production
endpoint, an operator can configure a separate test endpoint:

```http
PUT /api/workbench/test-profiles/acme/sandbox
Authorization: Bearer <operator token>
Content-Type: application/json

{
  "expected_revision": 0,
  "enabled": true,
  "rules": [{
    "tool": {"id": "lookup", "version": "1.0.0"},
    "endpoint": "https://sandbox.example.test/lookup",
    "credential_env": "AIDASH_SECRET_LOOKUP_TEST",
    "allowed_actions": ["read"],
    "allowed_resources": ["test-dataset"]
  }]
}
```

Set the named secret on the server before configuring the profile. The API
stores only its environment variable name. A Creator user sees compatible,
enabled profile IDs for an authorized draft; the list does not expose profile
rules or secrets. Running a real test requires `mode: "real"` and the selected
`profile_id`. Real tests are evidence about the chosen test connection, not
proof of the production integration.

## Factual Trust inspection

Trust reads an exact Registry version under `agent_version.inspect` on
`agent_version`. Its Policies view evaluates requested components against a
specific tenant, subject, and optional workspace, showing policy revision and
observation time. The Overview lists only authorized recorded use on the
connected node, authorized test metadata, and visible incidents. Incident
reading and management additionally use `agent_incident.read` and
`agent_incident.manage` on `agent_incident`. Reports export a point-in-time JSON
or printable HTML snapshot; private incident evidence content is omitted unless
explicitly selected and still authorized. Ordinary test payloads are not copied
to the report. Incident evidence is a fixed copy, retained while open and for
90 days after resolution by default. Expiry preserves source, integrity, and
deletion metadata. No Trust score, verdict, certification application, or
internal assessment is offered while external review arrangements are absent.
