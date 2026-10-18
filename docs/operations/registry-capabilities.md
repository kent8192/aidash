# Registry capability contract 1

Agent registration, generation templates, Workbench and native execution use Binding contract 1. Admission saves the complete immutable closure before activation; execution reads that closure and checks current authority and Provider compatibility at each protected boundary. See the [capability glossary](registry-capability-glossary.md) for the terms used here.

## Immutable declarations

Tools declare a registering Node, a versioned provider operation, a stable default model alias, a tier and supported narrowing. The provider owns effects, replay safety, fitting, continuation, disclosure and approval requirements. Generic HTTP/MCP calls remain `Unsafe` even if their transport configuration contains a stronger replay label. Native echo fixtures are not ordinary deployable integrations.

Bundle member IDs must be unique within each bundle, including across versions and Nodes, so an ID selection identifies one exact member. Root Agents and cluster coordinators reject undeclared member selections. Installing a bundle rewrites verified dependency identities and their qualifiers to the receiving Node. Published dependency graphs cannot contain system Built-ins; their Node-qualified declarations have no portable receiver substitution. Saved Run snapshots must match the Agent's normalized Binding closure, including selected bundle members, lifecycle companions, aliases, restrictions and origins. Snapshots retain the admission's remote/local placement and validate Provider contract digests and placement-derived exclusions when restored. Excluded defaults cannot carry a Provider implementation.

New Tool registrations require qualified Provider descriptors. Old transport-tagged definitions and old Agent configurations are unsupported for new execution; they remain historical records and are not converted. HTTP/MCP/Agent integration configuration is nested under the descriptor's `transport` field.

Every Tenant's Registry holds the built-in declarations at exact immutable versions (`aidash.<operation>@1.0.0`): the 17 legacy required/default operations, `capability_search`, `capability_describe`, `capability_load`, `capability_unload`, `skill_asset_read` and `task_assign`. Tenant creation writes them, and startup writes any that a Tenant lacks (ADR 0033). All are seeded whatever Exposure policies Agents use; an Agent's [Exposure policy](#deferred-capability-exposure) decides which of them it binds implicitly. Seeding verifies existing bytes and fails on a reserved-name conflict. These built-ins need no Catalog Approval and add no resource grants. Built-ins cannot be published, installed or mutated through Marketplace.

## Sandboxed and research built-ins

The Sandboxed tools (shell, code, package installs, outbound requests, patches and file sharing) and the research providers are built-in definitions too, but each Tenant approves them before use, because they act outside Aidash (ADR 0034). The Deployment offers a group only when it runs its provider: a Deployment without a runner offers no shell or Python group, and one whose `[research_providers]` settings do not declare a research provider offers no group for it. Startup and Tenant creation write the offered groups into every Tenant's Registry, unapproved.

| Group              | Bundle                           | Operations                                                           |
| ------------------ | -------------------------------- | -------------------------------------------------------------------- |
| `shell`            | `aidash.bundle.shell`            | `shell`, `shell_poll`, `shell_cancel`                                |
| `python`           | `aidash.bundle.python`           | `code_interpreter`, `python_install`, `python_poll`, `python_cancel` |
| `outbound_get`     | `aidash.bundle.outbound_get`     | `outbound_get`                                                       |
| `apply_patch`      | `aidash.bundle.apply_patch`      | `apply_patch`                                                        |
| `file_share`       | `aidash.bundle.file_share`       | `file_share`                                                         |
| `research_brave`   | `aidash.bundle.research_brave`   | `research.brave@1`: `search`                                         |
| `research_page`    | `aidash.bundle.research_page`    | `research.page@1`: `fetch`, `find`, `view`                           |
| `research_fixture` | `aidash.bundle.research_fixture` | `research.fixture@1`: `search`, `fetch`, `find`, `view`              |

Sandboxed operations have the `sandboxed` tier and research operations the `research` tier. Research entries are named `aidash.research.<provider>.<operation>@1.0.0` and use the default Model aliases `research_search`, `research_fetch`, `research_find` and `research_view` with the same schema for every provider, so a Tenant can replace one provider group with another without changing its Agents. Descriptors with a `core.` or `research.` provider cannot be registered by Tenants. See [Web research](../web-research.md) for the contract, evidence store, citation checks and disclosure approval.

A Tenant subject with `catalog.approve` approves a group by approving its bundle and every member in one `POST /api/authorization/{tenant}/catalog/approvals` request; a bundle's approval is refused while a member is unapproved. `task_assign` only starts a generation inside Aidash, so it is an ordinary built-in.

## Marketplace installations

A Tenant installs a Marketplace package as its own copy (`marketplace.install` and `registry.create`) and then approves and activates the installed revision. `POST /api/marketplace/installations/{id}/activation` and `POST /api/marketplace/approval-sets` are Tenant routes: they need `catalog.approve` on every definition they approve, in the caller's own Tenant. An approval set names the tenant and an exact array of installations (`installation`, `revision`, `digest`, `expected_activation_revision`) and approvals (`reference`, `expected_catalog_revision`). Dependencies outside the selection must already be active and approved. The operation never adds unreviewed approvals.

Every digest, revision, provider, dependency kind and lifecycle member is checked before approval writes. Conflicting or stale selections fail the entire transaction. Successful activation updates all selected catalog approvals and pointers together. The same request cannot reactivate a set using stale pointer revisions.

## Binding-only registration and execution

An Agent submits `schema_version: 1`, its exact `model` reference, `instructions`,
`bindings`, `remove_default` and an optional `exposure` policy. Each Binding has a
typed kind (`tool`, `bundle`, `skill`, `memory` or `source`), a Node-qualified
exact target, narrowing-only restrictions and, under a deferred policy only, an
optional `exposure`. Tool aliases are stable declaration names; collisions fail
after bundle expansion. The required `workspace_read` and `human_request` cannot
be removed. Under the legacy policy, bound Skills and Skill Sources require all
three Skill support tools; Skill Sources retain the canonical `skill_list`,
`skill_load` and `skill_read` aliases at admission and snapshot recovery. Under
`deferred@1`, bound Skills require `skill_asset_read`, and Skill Sources require
`capability_search`, `capability_describe`, `capability_load`,
`capability_unload` and `skill_asset_read`, all with their canonical aliases.
Cluster coordinators require all three coordination tools.

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
checking current authority. On the Legacy Projection Version a later boundary
observes current mutable content. On the Ordered Projection Version the
semantic observation is reused across steps while its Retrieval Key (query
inputs, Run-fixed retrieval budget, binding, authorization scope and the policy,
index and memory-binding revisions) is unchanged, and Skill context while the
Run's Skill record revision is unchanged; every reuse is rechecked first, a
revoked or narrowed authority pauses the Run, and a stale reuse retrieves again
once. Run inspection and state responses omit the Source observation cache;
durable storage retains it for authorized recovery.

Local execution, remote admission and Workbench save the same complete Binding
snapshot. Updating an active installation does not replace an admitted Run's
approved retained revision. Revocation or missing Provider support stops the
Run and retains its snapshot and journals. Restoring availability does not
resume work automatically; an explicit resume rechecks the saved contracts.
Direct capability HTTP requests recheck the selected Tool's current catalog
approval, pinned installation and immutable definition before dispatch.

## Deferred capability exposure

An Agent's `exposure` field names its [Exposure policy](registry-capability-glossary.md).
The policy is part of the Agent definition, so it is fixed with each Run's
Binding snapshot. Without the field, or with `{"version": "legacy@1"}`, every
model request carries every bound capability. Model requests, dispatch, Skill
records and errors are then exactly those of earlier releases, and
`skill_read@1.0.0` is unchanged. `legacy@1` accepts no other field.

```json
{
  "exposure": {
    "version": "deferred@1",
    "metadata_bytes": 4096,
    "schema_bytes": 16384,
    "skill_bytes": 32768
  }
}
```

| Budget           | Default | Accepted range | Measures                                                                                                                                                                   |
| ---------------- | ------- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `metadata_bytes` | 4,096   | 512–65,536     | JSON-escaped bytes of the capability index in the instructions                                                                                                             |
| `schema_bytes`   | 16,384  | 1,024–262,144  | Request JSON of every exposed tool definition as sent (`{"type":"function","function":…}` plus its delimiter), including Mandatory exposure, Eager bindings and companions |
| `skill_bytes`    | 32,768  | 1,024–262,144  | JSON-escaped bytes of the resident Skill blocks                                                                                                                            |

Any budget may be omitted and takes its default. Registration rejects an
out-of-range budget or an unknown field. Budgets are UTF-8 bytes of the JSON
request text, the same conservative estimate used by
[context compaction](../protocol.md#context-compaction); no tokenizer or model
call is involved.

### Bindings under `deferred@1`

A `tool`, `bundle` or `skill` Binding may set `"exposure": "eager"` or
`"exposure": "deferred"`; an absent value means `deferred`. Registration rejects
the field on other kinds and under the legacy policy. An Eager binding is in
the Exposure set from the Run's first request. An eager bundle makes its selected
members eager unless a member also has its own `tool` Binding that sets
`exposure`, which then decides. That Binding, with its alias and narrowing,
replaces the bundle's expansion for the member instead of conflicting with it.
Eagerness affects only exposure, never authority.

Normalization adds `capability_search`, `capability_describe`,
`capability_load` and `capability_unload` as Required Bindings with canonical
aliases; they cannot be removed. The implicit defaults replace `skill_list`,
`skill_load` and `skill_read` with `skill_asset_read`. `remove_default` may name
`skill_asset_read` only when no Skill Binding or Skill Source is bound, and
cannot name the legacy Skill tools. The legacy Skill tools cannot be bound
explicitly either, alone, in a bundle or through a Tool definition: they would
return Skill instructions without a Load or the `skill_bytes` budget, so
registration and snapshot recovery reject them. Under the legacy policy,
`remove_default` cannot name `skill_asset_read`, and the four `capability_*`
tools and `skill_asset_read` cannot be bound at all, since they read the
deferred catalog and would fail every call.

Mandatory exposure is `workspace_read`, `human_request`, the four
`capability_*` tools and, when bound, `skill_asset_read`. It is identified by
the bound operation, so `skill_asset_read` bound under a custom alias is still
Mandatory. Registration measures
Mandatory exposure, Eager tool definitions and eager Skill blocks, plus all
three budgets in full, against the selected model window with output and context
reserves. It also rejects any single tool definition larger than `schema_bytes`
and any Registry or attached Skill block larger than `skill_bytes`, naming the
alias and its size. If Mandatory exposure and Eager bindings exceed
`schema_bytes` or `skill_bytes` at a step, that step fails instead of running
with less.

### Requests

At each step the Harness rebuilds the Run's Discoverable capabilities, sorted by
alias. These are:

- every non-excluded aliased Tool Binding that passes its authority and Provider
  checks at that step, described exactly as dispatch advertises it (name = Model
  alias, parameters = declared schema, description = English declaration
  description, falling back to the Provider description);
- every Registry Skill, with its body taken from the Binding snapshot;
- on local Runs, every direct Skill (attachment or root).

Lifecycle companions such as `shell_poll` and `shell_cancel` are folded into
their parent Tool and exposed with it; they are not separate capabilities. A
companion shared by several exposed parents, such as `python_poll`, counts
against `schema_bytes` once. A run-message catch-up request sends only the
tools catch-up permits, and an interim media-intake request sends none; the
recorded usage counts only the tools sent. A
Skill's Model alias is `skill_<stem>_<hash>`. The stem is the Skill name or ID,
lowercased, with characters outside `[a-z0-9_]` replaced by `_` and truncated to
40 characters. The hash is the first eight hex digits of the SHA-256 of its
identity. Duplicate aliases fail the step.

A request's Exposure set has three parts:

- Mandatory exposure;
- Eager bindings that have not been unloaded;
- loaded capabilities whose alias, kind, identity and digest still match the
  catalog, in load order.

Only those tool definitions are sent. Every call of a response is checked
against the Exposure set of the request that produced it. A call to any other
alias returns a recoverable tool error and nothing is invoked: it tells the
model to use `capability_load`, or to retry after the next request when an
earlier call of the same response loaded it. The instructions carry each
resident Skill body, eager Skills first and then loaded Skills in load order:

```text
Skill <alias> (<identity>; digest <digest>; origin <registry|attachment|root>):
<SKILL.md body>
```

`<identity>` is `<registry_node>/<id>@<version>` for a Registry Skill and
`<origin>#<skill_id>` for a direct Skill. The legacy Registry Skill instructions
and `Pinned Skills` listing are not emitted. A Registry Skill with packaged
files ends its body with `Skill files are available through <tool> with alias
<alias> and digest <digest>. Read a listed path only when needed:` and one
`- <path>` line per file (`- <path> (binary; metadata only)` for base64 files),
where `<tool>` is the bound `skill_asset_read` alias; the legacy `skill_read`
guidance is not emitted. After the Skill blocks comes the
index. It lists every Discoverable capability outside the Exposure set, in alias
order, under a header naming `capability_search` and `capability_load`. Each line
is `<alias> [tool|skill]: <first description line, at most 160 characters>`.
Lines are added while they fit `metadata_bytes`; the rest are replaced by
`<n> more — use capability_search`.

Each request records `context.usage.exposure`:
`{"metadata_bytes", "schema_bytes", "skill_bytes", "exposed": [[alias, digest], …]}`.
Legacy requests omit it.

### Capability tools

These tools are `core.exposure@1` Built-ins. They read only the Run's own catalog
and change only the Run's Exposure set. Identities serialize as
`{"registry": {"registry_node", "id", "version"}}` or
`{"direct_skill": {"skill_id", "origin"}}`.

| Tool                  | Arguments                                | Result                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| --------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `capability_search`   | `query?`, `cursor?`                      | `{"results": [{alias, kind, name, description, digest, loaded}], "next_cursor", "truncated"}`. The query is split into lowercase alphanumeric tokens. Each token adds 2 for each of the alias and name that contains it and 1 for each of the description and bundle that contains it. Results with a positive score (all, for an empty query) are ordered by score and then alias, 16 per page. `description` is the first description line, at most 160 characters. `loaded` is true for anything in the Exposure set. |
| `capability_describe` | `alias`                                  | `{alias, kind, identity, digest, detail, bytes, budget}`. For a Tool, `detail` is the exact tool definition `{name, description, parameters}`. For a Skill, it is `{name, description, origin, license, files: [{path, digest, size}]}`. `bytes` counts the Tool definition with its companions, or the resident Skill block. `budget` is `schema_bytes` or `skill_bytes`.                                                                                                                                               |
| `capability_load`     | `alias`, `digest` (from search/describe) | `{"status": "loaded", alias, identity, digest, bytes}`; `{"status": "already_loaded", identity, digest}` for Mandatory exposure, a current Eager binding or a loaded capability                                                                                                                                                                                                                                                                                                                                          |
| `capability_unload`   | `alias`                                  | `{"status": "unloaded"}`; `{"status": "not_loaded"}` when the capability is not in the Exposure set                                                                                                                                                                                                                                                                                                                                                                                                                      |

| Code                       | Returned by            | Meaning                                                                                         |
| -------------------------- | ---------------------- | ----------------------------------------------------------------------------------------------- |
| `UNKNOWN_CAPABILITY`       | describe, load, unload | The alias is not a Discoverable capability of the Run; recoverable tool error                   |
| `MANDATORY_EXPOSURE`       | unload                 | Mandatory exposure cannot be unloaded; recoverable tool error                                   |
| `CAPABILITY_CHANGED`       | load                   | The supplied digest is not the capability's current digest; recoverable tool error              |
| `EXPOSURE_BUDGET_EXCEEDED` | load                   | Ordinary result `{"error", "exposed": [{alias, bytes}], "budget", "required"}`; nothing changes |

An invalid search cursor is a recoverable tool error. A description is recorded
whole or not at all: when it would not fit the remaining request budget, with
room reserved for the response's later calls, the step is deferred like a
`skill_read` that does not fit (`run.description_deferred`). The retained
context is compacted and the next request asks the model to retry the call.

A search page is recorded with only its leading results that fit the remaining
request budget, with the same reserve. A cut page sets `truncated` and
`budget_limited`, and its `next_cursor` resumes at the first dropped result.
When no result fits but the empty page does, the page also sets `deferred` and
a `message` to continue on a later turn. When not even the empty page fits, the
step is deferred (`run.search_deferred`) like a description.

A successful load or unload also returns an `exposure_update`. The Executor
stages it in `context.exposure` only for tools whose contract declares
`exposure_update`; tools never write the Run. The state is
`{"loaded": [{alias, kind, identity, digest, step}], "unloaded_eager": [alias, …], "pending": [update, …]}`,
with `pending` omitted when empty. Staged updates are applied in call order at
the next inference, so a load or unload takes effect from the next request and
never changes which calls of the current response are dispatched. Discovery,
load and unload decide against the state including staged updates: a capability
loaded earlier in the response reports `loaded` and `already_loaded`, and
counts against its budget. Applying an update is idempotent, including on
replay of a completed invocation. A loaded Skill's body is not returned
in the load result; it becomes resident in later requests. Unloading an Eager
binding records it in `unloaded_eager`, and loading it again removes that
record, so it counts against its budget again.

Loading never evicts. If the capability would raise exposed Tool definitions
above `schema_bytes`, or resident Skill blocks above `skill_bytes`, the result is
`EXPOSURE_BUDGET_EXCEEDED`. `exposed` lists the exposed capabilities of the same
kind with their bytes, `budget` the applicable budget and `required` the bytes
the load would need. The model must unload something before retrying.

### Skills, authority and placement

Under `deferred@1`, all Skill load state lives in `context.exposure`; the Run's
Skill record is never written. Under the legacy policy, `skill_load` still sets
the record's `loaded` flag. `skill_asset_read` (`core.skills@1`) takes `alias`,
`digest`, `path`, `offset?` and `max_chars?`. It reads one packaged file of a
Registry or direct Skill named by its Skill alias and current digest. Text is
returned in Unicode-scalar chunks capped by the `read_bytes` limit, with
`next_offset` and `truncated`; binary files, including every Registry file
declared `"encoding": "base64"`, return metadata only. A direct
Skill's `SKILL.md` is its instruction body, not a packaged file: it is absent
from `files` and reading it returns `SKILL_FILE_UNAVAILABLE`, so it becomes
resident only through `capability_load` under `skill_bytes`. A stale
digest returns the recoverable tool error `CAPABILITY_CHANGED` and an unknown
path returns `SKILL_FILE_UNAVAILABLE`; neither fails the step.

Loading never installs a package, grants authority or changes the Binding
snapshot. Every step still checks current authority and Provider support for
every Binding, and a failed check fails the step exactly as it does under the
legacy policy, whether or not the capability is loaded. `capability_*` add no
Agent flag, Core capability or resource requirement. `skill_asset_read`
authorization depends on the target: a Registry Skill requires its configured
Skill and `skill.use`, and a direct Skill requires the Agent's Skills Core
capability (see [authorization](../authorization.md#tenant-catalog-and-local-execution)).
An alias that names no Registry Skill, on an Agent without Skill attachments or
roots, returns the recoverable `UNKNOWN_CAPABILITY`.

The four capability tools and `skill_asset_read` are exposed to remote Runs.
Direct Skills are never Discoverable remotely; Registry Skills behave as under
the legacy policy. Remote headroom reserves the deferred budgets as registration
does.

A Workbench test of a `deferred@1` draft carries the session's Exposure set
across turns. Each request is selected as a Run selects it, with attachments
Discoverable and mounted roots absent. The `capability_*` tools are evaluated
against the pinned snapshot rather than fixtures, and calls to capabilities
outside the request's Exposure set are answered with the same recoverable
errors; both are recorded with the outcome `evaluated`. Load and unload results
apply from the next turn, and a continued session resumes the Exposure set its
evaluated results produced. A session never compacts its conversation, so
evaluated results are fitted before they are retained. A search page keeps only
its leading results that leave the next test request within the session's
input, model-window and total-token limits, as described above. A description
is kept only if that request also has room for the definition a load would add.
Otherwise the session omits its `detail` and sets `deferred` and a `message`,
and the digest still loads the capability. When not even an empty page or the
description without its detail fits, nothing is retained. The call is recorded
as `denied`, and the session stops with an error instead of sending a request
that exceeds its limits. Other tools still need fixtures or a real-tool profile.

To measure both policies on the same tools against a running server, see the
[capability exposure evaluation](../capability-exposure-evaluation.md).

## Home protocol and Human continuations

Peer execution requires protocol `0.2` or later; the `/v0.1/` URL namespace remains
a path name and does not negotiate an old contract. Both Nodes verify that the
protocol header is a version they accept, and the Provider contracts. Protocol
`0.3` additionally lets a remote Ordered Run see changes to the Home's semantic
corpus (see [context projection](../features/context-projection.md)). Remote mandatory workspace and Human operations
use Home authority under the exact task, grant, admission and Run. Unsupported
remote defaults have persisted placement exclusions; explicit incompatible
Bindings fail admission.

The `task_delegate` Provider contract is local-only, so its implicit default is
excluded on a remote executor. Calling a Home command directly does not restore
an excluded operation or broaden that contract.

Scoped Home Human requests are stored on the existing remote execution binding.
Deployment Administrator delegations keep their bounded journal on the existing task delegation,
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

## Foreign Agents

A Tenant reaches another Node's Agent only through scoped federation, with its
retained subject and mapping grants. The public foreign Agent Tool, which read
a peer's Deployment-wide Registry through the legacy discovery routes, is
removed with them (ADR 0043).

Agent Tool delegation from a Run uses scoped remote execution only. The
unscoped offer path that carried `delegations.binding_snapshot` is removed
(ADR 0043); the column keeps historical reservations readable.

## Workbench test connections

External HTTP/MCP test connections require a Deployment Administrator-owned profile whose
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
3. Deploy the Binding contract and peer protocol `0.2` or later across the fleet. Verify
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

A `memory` descriptor's configuration is the `ProviderConfig` in `crates/aidash-domain/src/memory.rs`: the engine and its policy only. It rejects unknown fields, so do not add operation, pinning, disclosure or binding controls to it. Those are separate Registry and admission controls: the memory operations are their own builtin Tool descriptors, the admitted Binding snapshot pins exact versions, authorization governs disclosure, and the Agent's Binding list selects the provider. Private memory bodies and credentials are not part of a descriptor: they stay in authorized provider storage, and catalog metadata never exposes them (see [Registry boundary](../architecture.md#registry-boundary)). The memory contracts that these providers serve are described in [architecture](../architecture.md#memory-contracts).
