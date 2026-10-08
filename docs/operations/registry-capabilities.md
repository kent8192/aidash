# Registry capability contract 1

Agent registration, generation templates, Workbench and native execution use Binding contract 1. Admission saves the complete immutable closure before activation; execution reads that closure and checks current authority and Provider compatibility at each protected boundary. See the [capability glossary](registry-capability-glossary.md) for the terms used here.

## Immutable declarations

Tools declare a registering Node, a versioned provider operation, a stable default model alias, a tier and supported narrowing. The provider owns effects, replay safety, fitting, continuation, disclosure and approval requirements. Generic HTTP/MCP calls remain `Unsafe` even if their transport configuration contains a stronger replay label. Native echo fixtures are not ordinary deployable integrations.

Bundle member IDs must be unique within each bundle, including across versions and Nodes, so an ID selection identifies one exact member. Root Agents and cluster coordinators reject undeclared member selections. Installing a bundle rewrites verified dependency identities and their qualifiers to the receiving Node. Published dependency graphs cannot contain system Built-ins; their Node-qualified declarations have no portable receiver substitution. Saved Run snapshots must match the Agent's normalized Binding closure, including selected bundle members, lifecycle companions, aliases, restrictions and origins. Snapshots retain the admission's remote/local placement and validate Provider contract digests and placement-derived exclusions when restored. Excluded defaults cannot carry a Provider implementation.

New Tool registrations require qualified Provider descriptors. Old transport-tagged definitions and old Agent configurations are unsupported for new execution; they remain historical records and are not converted. HTTP/MCP/Agent integration configuration is nested under the descriptor's `transport` field.

Node startup seeds the 17 required/default builtin declarations at exact immutable versions. Seeding verifies existing bytes and fails the entire transaction on a reserved-name conflict. System catalog visibility does not add tenant resource grants. Builtins cannot be published, installed or mutated through Marketplace.

## Pending Host packages

Configure `node.default_host_packages` with unique group names in the normal Node settings, for example `default_host_packages = ["shell", "python", "task_assign"]`. The operator's new-tenant policy request (`POST /api/authorization/{tenant}` with `expected_revision: 0`) stages the configured available groups in the same transaction as the new policy. Its response includes `pending_host_packages`, including unavailable reasons. If staging fails, neither the tenant policy nor any package is committed. Policy updates and startup never provision packages. An empty list preserves creation without package staging.

An authenticated operator can request the exact generated groups with `POST /api/marketplace/host-packages`:

```json
{
  "tenant": "example",
  "groups": [
    "shell",
    "python",
    "outbound_get",
    "apply_patch",
    "file_share",
    "task_assign"
  ],
  "idempotency_key": "00000000-0000-4000-8000-000000000001"
}
```

The response contains `installations` with exact immutable entry IDs, revision digests, dependency bindings and pending pointers, plus an `unavailable` map. Shell and Python include their poll/cancel operations. All persisted members share the operator transaction. An unavailable runner creates no sandbox revision and selects no fallback. Reusing the key preserves the receipt; retrying provisioning after a runner becomes available requires a new key. Existing installations and grants are not rewritten or activated.

## Review and activation

In Marketplace administration, select a tenant and prepare the desired Host groups. Inspect each pending definition and its dependency bindings, select every member to be approved and use **Approve and activate selected set**. The displayed selection retains the reviewed digest and approval revisions. If it becomes stale, reload the approval selection and review it again. Preparing or reloading a set grants no approval.

`POST /api/marketplace/approval-sets` requires the existing operator catalog-administration authority. Its input names the tenant and an exact array of installations (`installation`, `revision`, `digest`, `expected_activation_revision`) and approvals (`reference`, `expected_catalog_revision`). Include every pending member whose approval is being requested. Dependencies outside the selection must already be active and approved. The operation never adds unreviewed approvals.

Every digest, revision, provider, dependency kind and lifecycle member is checked before approval writes. Conflicting or stale selections fail the entire transaction. Successful activation updates all selected catalog approvals and pointers together. The same request cannot reactivate a set using stale pointer revisions. Individual approval/activation remains available through the existing API.

## Binding-only registration and execution

An Agent submits `schema_version: 1`, its exact `model` reference, `instructions`,
`bindings` and `remove_default`. Each Binding has a typed kind (`tool`, `bundle`,
`skill`, `memory` or `source`), a Node-qualified exact target and narrowing-only
restrictions. Tool aliases are stable declaration names; collisions fail after
bundle expansion. The required `workspace_read` and `human_request` cannot be
removed. Bound Skills and Skill Sources require all three Skill support tools;
cluster coordinators require all three coordination tools.

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Work within the authorized workspace.",
  "bindings": [
    {
      "kind": "memory",
      "target": {
        "registry_node": "aidash://example",
        "id": "conversation-memory",
        "version": "1.0.0"
      },
      "narrow": {}
    }
  ],
  "remove_default": ["task_delegate"],
  "max_steps": 64
}
```

Marketplace supports immutable native Memory/Source descriptor packages. Their
configuration cannot replace the adapter or its pinned references; private
documents remain separately stored and must be available with the exact digest
on the installing Node.

Memory reads require a Memory Binding and the admitted recall/reflect operations.
`memory_mutate` alone does not enable reading. Supported native
context declarations wrap the existing conversation/semantic memory, workspace
retrieval, private-reference, original-reference and Skill adapters. Private
Agent documents become immutable private Source definitions. Their contents
retain separate knowledge/reference authorization and are never made public by
Registry visibility. Inference journals a bounded content observation before
calling the model; recovery at the same boundary reuses that content after
checking current authority. A later boundary observes current mutable content.

Local execution, remote admission and Workbench save the same complete Binding
snapshot. Updating an active installation does not replace an admitted Run's
approved retained revision. Revocation or missing Provider support stops the
Run and retains its snapshot and journals. Restoring availability does not
resume work automatically; an explicit resume rechecks the saved contracts.

## Home protocol and Human continuations

Peer execution requires protocol `0.2`; the `/v0.1/` URL namespace remains a path
name and does not negotiate an old contract. Both Nodes verify the exact protocol
header and Provider contracts. Remote mandatory workspace and Human operations
use Home authority under the exact task, grant, admission and Run. Unsupported
remote defaults have persisted placement exclusions; explicit incompatible
Bindings fail admission.

The `task_delegate` Provider contract is local-only, so its implicit default is
excluded on a remote executor. Calling a Home command directly does not restore
an excluded operation or broaden that contract.

Scoped Home Human requests are stored on the existing remote execution binding.
Operator delegations keep their bounded journal on the existing task delegation,
and mirror each request under the receiver's real Run for Mesh inspection and
answer controls. Receiver answers are first committed at Home. Neither route
creates a shadow Run at Home. Lost replies replay the same request only for identical input.
Remote waiting Runs retain the Home request ID without a receiver-local foreign
key. Their Worker polls the Home journal at most once per second through the
current admission authority; local waiting Runs retain their ownership foreign
key. Apply both the physical and model-state continuation migrations before
restarting Workers.
The requester can inspect them in remote execution status and answer through
`POST /api/tasks/{task}/remote-grants/{grant}/human-requests/answer` with
`{ "id": "request-uuid", "response": { "answer": "Continue" } }`.
Current task/Human authority and a live grant are required; expiry, revocation,
foreign IDs and changed replay input fail. Unsafe-effect reconciliation uses
this same durable continuation and requires an explicit reconciled result.

## Public foreign Agent Tools

An operator's `integration.agent@1` descriptor may target an enabled protocol
`0.2` peer. Registration and admission authenticate
`GET /federation/v0.1/discover/{id}/{version}/bindings` and retain the receiver's
complete immutable Agent, model, cluster, bundle and Tool definitions. The parent
keeps its declared Tool alias; the child's Tools are not added to the parent's
model request. Live peer and receiver authority is checked again before model
assembly and each Tool effect. Missing peers, unavailable Providers and changed
closures fail without replacing any qualified reference.

This public protocol exports one receiver's operator-owned closure. Tenant
installations, generated Agents, Memory/Source/Skill context and nested foreign
references cannot be exported through it. Tenant admission never borrows an
operator peer credential; scoped federation continues to require its retained
subject and mapping grants. Explicit unsupported dependencies are rejected.

Agent Tool delegation stores `delegations.binding_snapshot` in the same
transaction as its target reservation. Retries send that original closure.
A receiver accepts a new Run only when its current admission snapshot matches
exactly; an idempotent replay must match the original Run snapshot. Updating a
catalog or retrying delivery cannot silently add a dependency or substitute a
same-named local Agent. The nullable forward migration preserves historical
unscoped reservations without importing legacy capability fields.

## Workbench test connections

External HTTP/MCP test connections require an operator-owned profile whose
`read_only_verified` field is true. Set it only after independently checking the
exact connection's supported operations and read-only contract. A publisher's
`transport.replay` claim does not establish this guarantee. Unattested external
calls require fixtures. Binding digests, the admitted draft, test profile and
current permissions are rechecked at every test step.

## Drained fleet rollout and rollback

1. Stop new admissions on every Node. Pause or drain existing Runs and reconcile
   uncertain effects with the existing software before replacing it.
2. Preserve historical definitions, grants, receipts, execution journals, private
   references and working objects. Apply the additive Home Human journal migration.
3. Deploy the Binding contract and peer protocol `0.2` across the fleet. Verify
   seeded system definitions and the deployment's admitted isolation profile.
4. Register fresh descriptors, explicit context definitions and Binding-only
   Agent versions. Prepare Host groups; review and approve their exact pending
   set, then approve Agents and grant exact descriptor/resource authority.
5. Validate selected tenant work through the native runtime acceptance gate and
   enable new admissions explicitly. Expand scope only after the evidence passes.

Old Agent configurations, `plugin_N` aliases and `builtin:*` grants are not
migrated. Existing tenants receive no automatic active installations. These
accepted scope decisions supersede the compatibility proposals in #105–#107.
They do not authorize deleting records or resetting a database. An old pending
Run requires re-registration and new admission; it cannot be resumed by silently
substituting a new graph.

For an operational rollback, disable admission while retaining the controller,
reconciliation workers and persistent journals/objects. Drain new-contract work
before rolling back software. Do not down-migrate, rewrite admitted snapshots or
resume old software against new-contract Runs. Runtime acceptance is per
architecture and deployment; portable tests and hosted CI do not certify a
production isolation profile.

## Native Memory providers

A `memory` Binding may target the immutable Rust Hindsight provider. Its exact extraction, derivation, reflection, embedding, reranker and tokenizer dependencies join the admitted closure. Native workspace `source` Bindings must share one exact provider, use compatible embedding configuration and fit its policy bounds; participant Sources must use the Agent's Memory provider. The Agent schema remains Binding-only. The `memory_mutate`, `memory_recall` and `memory_reflect` operations replace the historical `memory_write` operation. Cached native memory observations recheck current visibility before restart recovery uses them.
