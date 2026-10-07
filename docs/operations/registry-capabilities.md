# Registry capability contract 1

The implementation is in progress. The required cutover work below identifies the native execution and UI work that must finish before deployment. Portable resolver tests and Host staging acceptance cover the foundation; native Agent contract cutover still requires integration evidence. See the [capability glossary](registry-capability-glossary.md) for the terms used here.

## Immutable declarations

Tools declare a registering Node, a versioned provider operation, a stable default model alias, a tier and supported narrowing. The provider owns effects, replay safety, fitting, continuation, disclosure and approval requirements. Generic HTTP/MCP calls remain `Unsafe` even if their transport configuration contains a stronger replay label. Native echo fixtures are not ordinary deployable integrations.

Bundle member IDs must be unique within each bundle, including across versions and Nodes, so an ID selection identifies one exact member. Installing a bundle rewrites verified dependency identities and their qualifiers to the receiving Node. Saved Run snapshots must match the Agent's normalized Binding closure, including selected bundle members, lifecycle companions, aliases, restrictions and origins.

Until the native Agent cutover, Registry and Marketplace retain strict validation of existing transport-tagged Tool definitions. These definitions keep their original execution path and are not converted into Provider descriptors or admitted by the new Binding resolver.

Node startup seeds the 15 required/default builtin declarations at exact immutable versions. Seeding verifies existing bytes and fails the entire transaction on a reserved-name conflict. System catalog visibility does not add tenant resource grants. Builtins cannot be published, installed or mutated through Marketplace.

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

## Required cutover work

Complete new-schema registration, templates, Dashboard and workbench changes. Connect native admission to immutable Run snapshots before activation, and connect execution to those snapshots with current authority and provider checks. Add native context observations and Home-backed remote `workspace_read`/`human_request` admission. Verify source revocation and explicit same-snapshot resumption.

Drain existing work before fleet cutover. Existing Agent configurations, aliases and `builtin:*` grants are not migrated, and no active installation is manufactured from their old flags. Retain historical definitions, receipts and journals. Deploy only after the new Agent contract has native acceptance evidence; the current local work is not that evidence.
