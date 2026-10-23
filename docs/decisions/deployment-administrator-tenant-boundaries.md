# Deployment Administrator access to Tenant-owned data

Design for [Issue #301](https://github.com/kent8192/aidash/issues/301), dated
2026-10-23. The decisions below describe the target behavior. Implementation is
pending; this document does not claim that the current routes enforce it.

The inventory was read at `develop/0.1.0` commit
`429410771bb30e0e4db5d840789066dcbaeadc62` and compared with
`765508b0ff4bcbdf3a8e416afe800116719c9dc4`. The intervening changes concern CI
and release promotion, not these paths. The inventory is based on source and
documentation review, not an HTTP or runtime experiment.

## Decision

A Deployment Administrator reads or changes Tenant content only as a Subject of
that Tenant, through a browser Mapping or a Subject Credential supported by the
operation. Holding both a Mapping and a Deployment Administrator grant never
selects the Mapping implicitly: the request must use its Tenant authority.

The additional operational exception is a content-free safety control for a
known Run executed by this Deployment. It permits pause and cancel, exposes only
the minimum state needed to submit those commands safely, and records who acted
and why. It does not permit resumption, content inspection, or a cross-Tenant Run
list. A single Run is controlled in its Tenant's typed scope.

Peer Graph Grants are removed. Cross-Tenant Graph View uses ordinary Subject
authority and the receiving Tenant's Peer Mapping and policy.

### Approaches considered

| Approach                                                                                    | Trade-off                                                                                                                              | Decision |
| ------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| Tenant authority for content, plus a content-free Deployment Administrator safety control   | Retains a way to stop a runaway Run while keeping support access to content inside Tenant policy                                       | Selected |
| Retain direct Deployment Administrator content access, locally or through Peer Graph Grants | Makes support inspection convenient but discloses Tenant content, including Goal bodies, outside Tenant authority                      | Rejected |
| Require Tenant authority even to stop a Run                                                 | Removes the operational exception but leaves a Deployment Administrator unable to stop a known runaway Run without obtaining a Mapping | Rejected |

## Boundaries already decided elsewhere

[ADR 0043](../adr/0043-deployment-administrators-do-not-run-work-outside-tenants.md)
and [#243](https://github.com/kent8192/aidash/issues/243) already remove
Deployment Administrator execution. This design closes remaining content paths
and specifies the safety exception; it does not restore any execution route.

The explicit Deployment Administrator authorities retained by
[#363](https://github.com/kent8192/aidash/issues/363), #238, and #239 remain:
Deployment status and Tenancy Rules, the Tenant list and creation, identity and
grant administration, Invitations and Registration Requests, whole-policy
replacement, Tenant lifecycle administration, and the Deployment's Marketplace
administration. Their existing eligibility and deletion gates still apply.
Replacing a policy does not grant the administrator a content-reader context.

Tenant policy actions and initial groups remain as already defined. Ordinary
members retain their current execution, content, and Semantic actions.
Provider Credential management, retention decisions, and Workbench Tenant
settings remain controlled by the existing actions; `administrators` holds `*`.
No additional grant record or default member authority is introduced.

Internal workers are not Deployment Administrators. Retention, credential
reconciliation, identity-status processing, purge, and other background work
keep their separately justified scopes. Deployment resource ceilings remain
Deployment decisions; a Tenant chooses its values within those ceilings.

## Path decisions

"Move into a Tenant" means retaining the operation under its existing Tenant
policy action and removing the Deployment Administrator bypass. It does not mean
that a Deployment Administrator grant becomes a Tenant policy allowance.

| ID  | Path                                                                                                     | Decision and reason                                                                                                                                                                                                                  | Implementation track               |
| --- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------- |
| P01 | Local Graph View, including Goal bodies                                                                  | Move into a Tenant. Graph projection is content access, even when many other body fields are omitted                                                                                                                                 | I1                                 |
| P02 | Remote Graph View through a Peer Graph Grant, and grant list/set                                         | Remove the special grant and administrator projection paths. Use normal Subject federation and the receiving Tenant's policy                                                                                                         | I3                                 |
| P03 | Workspace events, event SSE, and Run inference-progress SSE                                              | Move into a Tenant. Replay and live delivery must enforce the same authority as the underlying Workspace and Run                                                                                                                     | I1                                 |
| P04 | Run details, Research usage, and cited evidence                                                          | Move into a Tenant. These disclose execution context and evidence, not infrastructure status                                                                                                                                         | I1                                 |
| P05 | General `run_control`, including resume                                                                  | Move into a Tenant. Both administrator browser and shared bearer authority are refused; administrator pause/cancel use P06 instead                                                                                                   | I1                                 |
| P06 | `GET`/`POST /api/runs/{id}/management`                                                                   | Keep a narrowed safety path. Permit only the metadata and pause/cancel contract below, with Tenant scope and durable audit                                                                                                           | I2                                 |
| P07 | Provider Credential list/detail, OAuth connect/reconnect, Binding reads/changes, revoke/delete           | Move into a Tenant. Metadata reveals a Tenant's provider setup, while changes affect its execution and spending                                                                                                                      | I1                                 |
| P08 | Workbench inspection, drafts, incidents, audit, permission context, and reports                          | Preserve the #243 inspection/draft denials. Move incident reads/writes/events, catalog audit, permission context, and reports with copied incident evidence into Tenant authority; administrator lists cannot omit the Tenant filter | I1; preserve existing #243 denials |
| P09 | `/api/state`, Workspace snapshots, Task lists, artifacts, conversations, and human requests              | Move Tenant collections into Tenant authority. Deployment state may expose its access profile and Deployment identity/infrastructure, with no Tenant content collections                                                             | I1                                 |
| P10 | Task abandon                                                                                             | Move into a Tenant. This changes Tenant work and is broader than the Run safety exception                                                                                                                                            | I1                                 |
| P11 | Channel history, attachment downloads, and Channel writes                                                | Move into a Tenant. Audit the shared access lease and API-token routes as well as browser restrictions. Any remaining administrator execution is a correction to #243, not a new exception                                           | I1                                 |
| P12 | Knowledge entries/search/history, Semantic Index configuration/status, and knowledge mutations           | Move into a Tenant. #360 explicitly left its administrator path for this decision; content and settings use their existing policy actions                                                                                            | I1                                 |
| P13 | Workbench Tenant Test Profiles and Test Limits                                                           | Move into a Tenant. The Tenant selects these within Deployment ceilings; draft-specific controls continue to require draft authority                                                                                                 | I1                                 |
| P14 | Research Retention Rules, their history, Evidence Revocation, and revocation records                     | Move into a Tenant. The Tenant decides retention within Deployment ceilings and whether to revoke its evidence; workers apply those decisions                                                                                        | I1                                 |
| P15 | Tenant audit history during deletion and after purge                                                     | Close ordinary audit APIs while deleted or purged. A valid restore re-enables ordinary Tenant-authorized reads; retained history is erased under `audit_retention_days`                                                              | I1, coordinated with #368          |
| P16 | Agent Generation policies, assignments, requests, history, usage, specifications, and lifecycle controls | Move into a Tenant. Policy configuration, request reasons and generated specifications are Tenant content, and every mutation requires its existing Tenant action                                                                    | I1                                 |

### Inventory anchors

These are paths in the inspected baseline, not a substitute for a fresh inventory
at implementation time:

- `server/src/apps/execution/services/management.rs`: cross-Tenant `state`,
  Run details/control, events/streams, Research usage and evidence operations.
- `server/src/apps/workspaces/services/management.rs` and
  `services/access.rs`: Workspace/Task operations and the shared Channel lease.
- `server/src/apps/workspaces/sse/services/connection.rs` and `inference.rs`:
  administrator stream authority.
- `server/src/apps/identity/services/execution/management.rs`: existing
  content-free Run management, with pause/cancel only.
- `server/src/apps/identity/services/provider_credentials.rs`: the shared
  administrator allowance affects more than listing.
- `server/src/apps/knowledge/repositories/access.rs` and
  `services/service.rs`: knowledge and Semantic access without Subject policy.
- `server/src/apps/registry/workbench/repositories/authority.rs`, `profile.rs`,
  and `sandbox.rs`: Tenant settings with an administrator allowance.
- `server/src/apps/execution/generation/urls.rs`, `services/requests.rs`, and
  `repositories/policy.rs`, `reads.rs`, and `controls.rs`: Agent Generation
  policy list/set, assignment, request reads/history/usage/specification and
  lifecycle control. Include the portable `generation::{policy,reads,lifecycle}`
  gates and authenticated foreign-generation request/cancel paths.
- `server/src/apps/registry/workbench/urls.rs`,
  `repositories/incidents.rs`, `audit.rs`, `permissions.rs`, and `report.rs`:
  incident creation/list/detail/update/events, catalog audit, permission context
  and reports. Trace the shared application gates and copied evidence as well
  as the native adapters; these routes are not covered by draft/inspection denial.
- `server/src/apps/identity/repositories/retention.rs` and
  `server/src/apps/execution/repositories/research/revocation.rs`: administrator
  retention and revocation decisions.
- `server/src/apps/identity/services/peer/graph.rs`,
  `repositories/graph/persistence.rs`, and
  `crates/aidash-application/src/federation/graph/visibility.rs`: Peer Graph Grants
  and administrator projection.
- `crates/aidash-application/src/registry/workbench/inspection/usage.rs` and
  `registry/workbench.rs`: the effective non-Subject denial for inspection and
  drafts. Residual native administrator branches do not prove a reachable read.
- `server/src/apps/identity/services/http_auth.rs`: browser write restrictions
  differ from shared bearer behavior. Both authentication paths need coverage.

The current required-Home exclusion protects some remote-derived content; it is
not a general Tenant policy check. Existing receiver-side Graph code also does
not establish that the current browser outbound Graph flow succeeds. These
decisions apply to reachable and dormant administrator bypasses alike.

## Run safety control

### Authority and metadata

Use the existing Run management route as the safety entry point. A request names
both the expected Tenant and Run ID. A mismatched or absent Run exposes no
information about another Tenant. No administrator search, enumeration, Run
list, or content-bearing stream is added.

An administrator GET returns only the Run ID, typed phase, typed control, and a
control revision. It must not serialize `RunInspection` or reuse the current
`RunManagement` diagnostics wholesale. Free-form errors, Goal/Task content,
Context, Agent/model identifiers, evidence, semantic/context recovery details,
and memory cleanup details require ordinary Tenant authority.

Both an authenticated named browser Deployment Administrator and the shared
recovery API token may stop a Run. Browser session/grant revalidation, CSRF and
Origin checks remain required. The shared token is recorded as the shared bearer
actor; it never claims to identify an individual.

Tenant Subjects keep the supported management operations under their existing
Workspace and `run.control` checks. Supplying administrator-only fields does not
turn a Subject into a Deployment Administrator.

### Commands, retries, and receipts

An administrator command carries the Tenant, Run ID, `pause` or `cancel`, the
expected control revision, an opaque operation ID, and a required reason.
The control revision detects intervening control decisions, including Tenant
resumption and internal pause/resume. Unrelated worker progress must not make an
otherwise current safety command stale.

The command validates current authority, Tenant availability, the target binding,
and input before replaying a stored receipt. Within that boundary:

1. An identical retry by the same caller returns its original receipt and has no
   second effect. Reusing its operation ID for different input conflicts.
2. A new command for a terminal Run changes no state and returns a terminal no-op
   receipt. An already-cancelled control cannot be downgraded to pause or active.
3. Other new commands require the expected control revision; a mismatch returns
   409 and requires a fresh read.
4. The control change, immutable audit record, and replay receipt commit together.
   A failure to persist the audit rolls back the command. Notify workers only
   after commit.

Receipts identify the operation, target, requested action, and accepted/no-op
outcome. They describe that operation, not the Run's current state after later
commands. An exact replay does not reinstate a pause after an authorized resume.

### What pause and cancel mean

- Pause prevents subsequent scheduling. It does not interrupt an inference or
  external call already in progress.
- Cancel requests interruption of current work through existing worker and
  provider cancellation mechanisms. It is not a promise that an external system
  rolls back an already-submitted operation.
- A receipt acknowledges the durable request; it does not claim that execution
  has already stopped. No hard cancellation latency guarantee is introduced.
- A safety pause is a manual pause. Automatic recovery from an identity-status
  outage must not resume it. Resumption requires fresh Tenant authority through
  the ordinary Run control path. No administrator release/resume operation or
  separate safety hold is introduced.

### Target and lifecycle boundaries

The safety path applies to Runs executed by this Deployment, including receiver
Runs whose content is hidden by required-Home rules. It does not forward an
administrator command to stop a Run executed by another Deployment.

A deleted Tenant is refused with `410 tenant_deleted`, including on retries;
the safety path cannot undo Tenant Deletion. Deletion and control must serialize
so that a successful deletion cannot be followed by a new accepted safety
mutation. A missing/purged Run reveals no additional data. Existing restore and
purge administration remain governed by their own endpoints.

The operation uses `TenancyScope::Tenant`. If the existing schema first needs
to resolve a Run's Tenant from its ID, keep that lookup read-only under the
existing `RunTenant` reason, check the caller's expected Tenant, and perform the
operation in that Tenant. Never open a general read/write Deployment Scope for
a single Run.

### Audit visibility and lifetime

Record Tenant, Run ID, action, actor kind, browser Identity ID when present,
reason, timestamp, operation ID, and accepted/no-op outcome. No record contains
execution content or copied credentials. The required reason is visible to the
Tenant's audit readers, so it is not a private support note.

Expose the records to the Tenant under its existing `decision.read` audit
authority. A Deployment Administrator does not gain access to the Tenant's audit
history through this safety exception; its retry receipt remains the narrow
operation acknowledgement described above.

Classify safety audit and committed receipt storage explicitly. Keep successful
receipts throughout the target Run's lifetime so an old operation cannot acquire
a second effect. Retained audit uses the existing Tenant audit lifecycle and
`audit_retention_days` after purge. A retained record must not depend on a
foreign key to a purged Run or Workspace.

During Tenant Deletion and after purge, ordinary audit APIs are closed to both
Tenant callers and Deployment Administrators. A valid restore during the grace
period re-enables the normal checks. Restore, purge preview, and lifecycle
operations expose only the minimal information required by those existing
administrative operations. Retention alone creates no API read authority.

## Implementation conventions and remaining builder choices

The following conventions make the contract concrete without changing Tenant
policy defaults:

- Use a dedicated typed administrator metadata/receipt projection and a typed
  safety command, rather than optional authority bypasses in content services.
- Bound the required reason to 1–1000 trimmed characters, using the existing
  Evidence Revocation reason rules as the validation precedent.
- Scope operation IDs by Tenant and authenticated caller, with the Run and full
  command frozen in the receipt's input. Shared bearer retries share that caller
  kind. Recheck current authority and deletion status on every retry.
- Use the existing finite Tenant audit lifecycle for the new record types and
  include them in table classification and purge verification.

The implementer chooses exact serializer names, query parameter/body placement,
receipt outcome names, storage layout, indexes, and the control-revision
representation. These choices must preserve the field allowlist, atomicity,
idempotency, deletion checks, and concurrency rules above. Layering follows the
repository boundaries: pure rules in domain, use cases/ports in application,
execution lifecycle in harness, and HTTP/persistence composition in server.

## Implementation tracking

The following Issues track implementation; #301 is the decision record. All
three carry `bug`, `breaking-change`, and `severity:high`.

| Track | Proposed Issue title                                                                                                                             | Completion boundary                                                                                                                                                    |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| I1    | [#484: Enforce Tenant authority on remaining Deployment Administrator content and settings paths](https://github.com/kent8192/aidash/issues/484) | P01, P03–P05, P07–P16; preserve P08 inspection/draft denials and enforce its remaining content boundaries                                                              |
| I2    | [#485: Make Run management an audited content-free safety control](https://github.com/kent8192/aidash/issues/485)                                | P06, including minimal GET, expected Tenant/control version, retries, receipts, audit, lifecycle gates, and same-release refusal of ordinary administrator Run control |
| I3    | [#486: Remove Deployment Administrator Peer Graph Grants and remote Graph bypasses](https://github.com/kent8192/aidash/issues/486)               | P02, including grant persistence/routes, administrator protocol branches, and #241 design reconciliation                                                               |

Enable P06 only in a release that also refuses every ordinary Deployment
Administrator `/api/runs/{id}/control` operation, including shared-bearer resume.
If I2 lands before the rest of I1, I2 must carry that refusal; otherwise deliver
the control changes atomically. No intermediate result may retain resume as an
administrator fallback. The full boundary is complete only after all three
tracks meet their acceptance criteria. A coherent implementation PR may close
multiple tracks where shared authorization code makes separate delivery risky.

Coordinate P15 and all retained safety deletion gates with
[#368](https://github.com/kent8192/aidash/issues/368). Verify its full acceptance
criteria rather than closing it merely because content routes were removed.
Reconcile I3 with the active #241 work and
[Nodes and Federation](nodes-and-federation.md), especially section 3.6 and open
point 10.5. Remove the Peer Graph Grant consequence from ADR 0064 and record the
safety exception alongside ADR 0043. Update authorization, Provider Credential,
protocol, dashboard, OpenAPI, and contract documentation with the implementation;
the current runtime documentation remains a description of the current code
until those changes land.

## Acceptance and verification

- Inventory every request-reachable Deployment Administrator allowance,
  including helper-based non-Subject fallbacks. Classify it as an existing
  administrative authority, the exact safety exception, or a denied content path.
  A role arm alone does not prove that the enclosing application gate allows it.
- Exercise administrator browser and shared bearer requests, an authorized
  Mapping/Subject Credential, an unauthorized same-Tenant Subject, and a second
  Tenant. Existing operation-specific browser/credential restrictions remain.
- Assert refusal before content serialization or effects on moved paths. Cover
  cross-Tenant collections, events/SSE replay and revocation, attachments,
  knowledge mutations, Credential/Binding/OAuth operations, and Tenant settings.
  Cover Agent Generation policy reads/writes, assignments, request lists, history,
  usage, specifications, lifecycle and foreign-generation controls before any
  content is serialized or effects occur.
- Preserve Workbench inspection/draft denials. Cover incident create/list/detail/
  update/events, catalog audit, permission context, and reports with copied
  evidence; require Tenant filtering even for collections. Ensure no empty-list
  or alternate repository path becomes a content disclosure or a write bypass.
- Exercise the release containing P06 with both browser and shared-bearer
  requests to ordinary `/control`, including resume, and prove refusal before
  mutation or content serialization. This is part of I2 even if I1 is incomplete.
- Safety tests must show the response allowlist, wrong-Tenant refusal, mandatory
  reason, terminal/cancelled no-ops, version conflicts, identical replay, changed
  input conflict, and an old retry after Tenant-authorized resume without a
  second effect. Verify rollback when audit persistence fails.
- Cover required-Home receiver safety, shared-token actor attribution, named
  browser attribution, deletion/control and retry/deletion races, Tenant audit
  permissions, and closed audit APIs during deletion and after purge.
- Verify manual pause is not automatically resumed, pause does not claim to
  interrupt an ongoing inference, and cancel acceptance is distinct from its
  eventual worker outcome. Use observable behavior rather than assumed timing.
- Remove Peer Graph Grant APIs, administrator federation token/projection paths,
  schema/contracts/UI, and stale design claims. Preserve Subject-based federated
  Graph behavior, receiver authorization, and revocation checks.
- Run the relevant application/server authorization and federation checks,
  schema/classification/purge checks for changed storage, generated API/contract
  checks, and dashboard checks for touched flows. No runtime acceptance is claimed
  by this design document.
