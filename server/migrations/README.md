# Aidash native migration history

`federation/0028_run_only_tool_answers` (after `federation/0027_federation_tool_lists_state`)
adds the nullable `jsonb` column `run_tool_allowance` to `authorization_remote_admissions` (the
tools the receiving Node's people allowed for one Run only) and `tool_request_additions` to
`authorization_remote_grants` (the tools the Home's people allowed for one Run only), each a
Federation tool list of ADR 0139 (#989); NULL means none. It uses only Reinhardt operations.
`0029_run_only_tool_answers_state` records the model state.

`federation/0026_federation_tool_lists` (after `federation/0025_federation_token_ids_state`)
adds `tool_request` and `tool_allowance` (nullable `jsonb`; NULL means the default list) and
`tool_block` (`jsonb NOT NULL DEFAULT '[]'`) to `node_registrations` and
`node_registration_history`, the Federation Tool Request, Allowance and Block of ADR 0139 (#989).
It uses only Reinhardt operations. `0027_federation_tool_lists_state` records the model state.

`federation/0024_federation_token_ids` (after
`federation/0023_delegation_unused_columns_state`) creates the Tenant-owned
`federation_token_ids` table, which records the `jti` of every Federation token a
receiving Node in another Tenant verified until `exp` plus 30 seconds (ADR 0064,
#989). Its row-level policy is a SQL asset because Reinhardt migrations cannot
express policies. `0025_federation_token_ids_state` records the model state.

`federation/0022_delegation_unused_columns` (after
`federation/0021_node_preview_origin_merge`) drops the `delegations_retry` index
and the `binding_snapshot`, `human_requests`, `next_attempt_at` and `delivered`
columns of `delegations`, which nothing writes or reads since outbound delivery
and the foreign Agent closure import were removed (#373). A delegation row now
records only the agent version that runs a locally delegated Task.
`0023_delegation_unused_columns_state` records the model state. Both use typed
operations; the reverse restores the columns empty.

`identity/0068_decision_coalescing` adds `run_id`, `remote_grant_id` and
`coalesced` to `authorization_decisions`. Existing rows name neither and stay as
they are. `identity/0069_decision_coalescing_indexes` is database-only and adds
the partial unique indexes that keep one coalesced allow per Run or Remote Grant,
subject, action, resource and policy revision (ADR 0116). Both use typed
operations.

`execution/0120_event_waits` (after `execution/0119_egress_request_times`) requests Run activation in the transaction that
finishes an operation or changes an owned Task to a terminal or blocked state.
PostgreSQL trigger functions require SQL assets because
Reinhardt migrations cannot express their bodies.

`execution/0113_content_object_references` is a destructive undeployed
content-store cutover. It discards UUID-backed objects, resets existing Working
Area manifests, deletes Thread Attachment rows whose original or WAV object ID
is set, and deletes `reference`, `transfer_in`, `transfer_out`, `collaboration`,
and `cleanup` core records that can name those objects. Inline-only attachments
and unrelated records remain. Its owner-only SQL asset opens Deployment Scope
before writing Tenant rows. The reverse restores the legacy schema and clears
rows naming content-store objects; it cannot restore discarded bytes. Restore a
pre-cutover backup to recover data.

`registry/0057_embedding_inputs` through `0059_reference_image_byte_state`
add image retrieval inputs, private reference images, model state and Tenant
policies after `0056_attachment_tools`. `execution/0092_reference_embedding_usage`
and `0093_reference_embedding_usage_state` record reference embedding usage after
`0091_bounded_read_indexes`.

`execution/0109_scope_egress` (after `0108_node_preview_merge`) adds Node-owned scope state, per-day counters,
connection metadata and stop events. Its composite references bind every child
to the same Tenant, Node and scope, and Node deletion cascades through them.
`0110` applies forced Tenant row policies; Reinhardt cannot express those
policies as schema operations, so the migration uses SQL assets. `0111` records
the corresponding model state.

`registry/0056_attachment_tools` (after `registry/0055_remove_structured_harness_contracts`) extends the existing PostgreSQL descriptor
and Agent default validators for the built-in `attachment_copy` operation.
The function bodies use SQL assets because Reinhardt has no typed operation
for replacing PostgreSQL functions. Reversal refuses while definitions or Run
snapshots still use the new operation.
`registry/0054_remove_agent_cluster` removes the obsolete Agent config key from Registry definitions, Agent drafts and registration requests, then removes it from the Agent contract. `execution/0089_remove_agent_cluster` removes it from Generation policy templates and request definitions. PostgreSQL JSON updates and function replacement use SQL assets because Reinhardt migrations cannot express these transformations. Both are irreversible. Each owner-only atomic migration establishes Deployment Scope before its first row access because the native migration executor does not open scoped application transactions. The registry update temporarily disables the Marketplace writer fence within its transaction, retaining its other validation and reference triggers. Installed Registry copies change while immutable Marketplace versions and revisions stay untouched: old Marketplace versions must be republished and old installation revisions may no longer match. Run and Federation snapshots are unchanged; Runs using old snapshots fail and need cancellation. Surviving Run constraints and triggers do not call the Agent validator, so cancellation updates do not revalidate the retained snapshots. Catalog Approvals and Federation pins covering changed definitions need renewing.

`execution/0081_lease_recoveries` updates the physical `runs.pending` default,
and replaces the activation trigger function. PostgreSQL function replacement
is a SQL exception that Reinhardt cannot express. The server decodes the
former Lease Recovery flag of existing Runs, so the migration writes no Run
rows; rollback does not rewrite stored counters for older binaries.

Issue #296 adds write-only Tenant Secrets in `identity/0036` through `0038`
and durable operation egress budgets and audits in `execution/0046` through
`0048`. Their forced Tenant row policies use the existing policy renderer.
`registry/0042_external_services` extends both Registry kind checks and the
HTTP/MCP and Agent Binding contracts; `0043` records the ORM enum state. Kind
constraints use typed Reinhardt operations. PostgreSQL function replacements
remain SQL DDL assets because Reinhardt has no function-body operation. The
reverse restores the previous Tool and Agent contracts; remove External
service definitions and bindings before reversing the kind constraints.

`identity/0041_catalog_restriction` adds a nullable `restriction` jsonb column
to `authorization_catalog` and to its append-only history (ADR 0029, ADR 0095).
The column names subjects and Assignable Groups that may use one exact version
on one Node; NULL keeps the pre-existing meaning, everyone the Tenant's policy
allows, so every existing approval and history row migrates as unrestricted.
The columns are typed `AddColumn` operations, so the model state follows the
physical migration and no state-only checkpoint is needed.

`identity/0042_catalog_approval_requests` creates the Tenant-owned request
table for ADR 0096, its `0043` row policy and its `0044` state-only snapshot.
The physical table has a partial unique index keeping at most one open request
per requester, root and Node, and a check that only a declined request carries
a decline reason. The policy asset follows the standard Tenant template; the
state snapshot carries the columns without the physical checks and index.

Reinhardt is the sole migration owner. The source files are grouped under the
labels `identity`, `registry`, `workspaces`, `execution`, `federation`, `knowledge`,
`marketplace`, and `operations`; these are modules in `aidash-server`, not separate
Cargo packages.

## Community and Enterprise migrations

This directory holds the Community build's apps. The Enterprise build adds its
own apps' migrations under `enterprise/server/migrations/` (ADR 0032), such as
`gcip/0001_admissions`. Enterprise tables may reference Community tables;
Community tables never reference Enterprise ones, and the Community schema has
no Enterprise table. The `Build` descriptor names each build's extra apps and
their migration roots relative to `core.base_dir`, so the Enterprise image
places them at `/app/enterprise/server/migrations` beside
`/app/server/migrations`. `require_current` reads every root. Reinhardt's
`migrate` and `makemigrations` read one directory and keep SQL assets inside
it, so `aidash migrate` stages a copy of every root's apps when a build has
more than one. `enterprise/server/tests/migrations.rs` runs
`makemigrations --check` over the same union.

`makemigrations` writes into the directory it reads, so a new Enterprise
migration is generated in a staged project and copied back: copy every app
directory of both roots into `<project>/migrations`, copy
`enterprise/server/src/bin/manage.rs` to `<project>/src/bin/`, give it settings
with `[deployment] offering = "cloud"`, run the Enterprise `manage
makemigrations <app> --state-source files` from `<project>`, and move the new
file into `enterprise/server/migrations/<app>/`.

## Supported databases

The initial migration accepts an empty PostgreSQL database. A database already
initialized by this native history supports normal replay and subsequent native
migrations. Existing SeaORM databases and experimental migration-branch databases
are not adopted or reset. Unknown records and applied migrations with missing
dependencies cause startup to stop.

The baseline preserves the schema at Aidash revision
`d1201622a4110a5d4fb908a15025752f9cae10d2`, including the typed Run state changes
from PR #100. The [original migration sources](https://github.com/kent8192/aidash/tree/d1201622a4110a5d4fb908a15025752f9cae10d2/migration)
remain available at that immutable Git revision; the old migration crate and
monolithic Rust source are removed from this workspace. Those 54 legacy migrations
are consolidated by object ownership and phase in the native app history. The
native migration files are the authoritative schema and identity source; no
second manifest translates or imports the old migration ledger.

The merged `develop/0.1.0` revision
`d75d1c0453a6e8bd267e428cf2303dd7c6ff8639` adds legacy migration 55,
[`m20261002_000000_desktop_sessions`](https://github.com/kent8192/aidash/blob/d75d1c0453a6e8bd267e428cf2303dd7c6ff8639/migration/src/m20261002_000000_desktop_sessions.rs).
Its two desktop tables and three session columns are recorded in
`identity/0009_desktop_sessions`, generated with the real Reinhardt
`makemigrations` command as a state-only snapshot. At the pinned revision, generic
byte-vector metadata emits PostgreSQL `BINARY`, and the ORM macro rejects an
explicit `bytea` field annotation; see
[#6637](https://github.com/kent8192/reinhardt-web/issues/6637).
`identity/0008_desktop_schema` applies the matching physical schema with
native typed `FieldType::Bytea` columns, original column order, named unique key,
cascading session references, and two lookup indexes. This follows the baseline's
existing state/database split and remains one graph and one ledger. The physical
migration precedes its state snapshot. File-state replay excludes database-only
operations from logical model reconstruction while retaining their dependencies;
the pinned revision incorporates
[#6638](https://github.com/kent8192/reinhardt-web/issues/6638). The final snapshot
records the registered model metadata. These migrations extend the original table definitions and preserve their SQL
assets. The environment migration additionally records conditional extension
ownership before this new native history is published. The complete native graph contains 58
records and describes 119 models.

The original baseline has 36 migrations for physical schema creation and eight
`0007_model_state` snapshots. The desktop addition brings this to 37 physical
migrations and nine state snapshots. The physical graph orders functions,
tables, keys/indexes, references, seed data, and triggers across app boundaries.
The original eight snapshots describe 115 ORM models; the desktop snapshot adds
two models. These are **state-only**: they do not recreate tables or replace
procedural constraints.

Physical table definitions, named primary/unique/foreign keys, CHECK constraints,
generated columns, named ordinary/expression/partial indexes, and column defaults
use `reinhardt::db::migrations::Operation`. SQL expressions in CHECKs,
defaults, index predicates, and generated-column metadata are expression bodies;
whole supported CREATE/ALTER statements must not be passed to `RunSQL`.

Historical DDL assets live in each app's `sql/forward/` directory and are loaded
with `include_str!` into `Operation::RunSQL`: procedural functions/triggers/DO
blocks, the frozen sequence and ALWAYS identity definitions, transaction-local
session settings and extension ownership markers. Every
physical migration sets `search_path` to `public, pg_catalog` locally because the
pinned typed operations accept unqualified names; this preserves the frozen
public schema even with a custom connection search path.
Corresponding reverse SQL assets live in `sql/backward/` and are included in
`RunSQL.reverse_sql`. Independent function/trigger definitions in one migration
share a single SQL asset and operation; sequence/table dependency boundaries remain
ordered. Independent sequences are created together before their tables; column
defaults are part of typed table definitions, and ownership/identity configuration
is grouped after the tables. This retains the baseline's original column and
sequence reversal order. Reverse scripts drop triggers/functions in reverse order,
remove identity configuration, detach/drop explicit sequences, and delete only
baseline seed rows.
Transaction-local settings have no persistent inverse; their backward files use
an explicit no-op DO block. The initial native migration creates `pg_jsonschema`
using a conditional procedural operation with an explicit ownership marker. An
administrator may provision this extension in an empty database before the
application migrates using its scoped role. Reversal preserves borrowed
extensions and drops only extensions created by this history.
A final context operation sets the public search path before typed reverse operations run.

The conversation Scratch Space backfill is generated with Reinhardt Query by `workspaces::services::migration_backfill`. The `migrationseeds` command checks its forward and reverse assets for drift. It assigns each pre-existing conversation a dedicated space with the conversation creator and default settings.

The two baseline seed mutations and their inverses are built with Reinhardt Query
in the owning federation and marketplace apps. `manage migrationseeds --check`
compares their rendered PostgreSQL statements with the four canonical SQL assets.
`manage migrationseeds --write` regenerates the registered seed and backfill assets without reading
runtime credentials or opening a database. Regeneration is for an **unapplied**
history; never rewrite an applied migration. The native filesystem source parses
static Rust syntax without executing arbitrary function calls, so these generated
assets remain literal `include_str!` payloads in the native graph. This is
materialized Query output, not a handwritten DML exception or a second migration
engine. The forward seed assets also set the transaction-local search path or deployment scope where needed. Regression checks load the exact generated SQL through
`FilesystemSource` and verify both initial gate values after migration and after
complete reversal/reapplication.

`execution/0090_run_created_at` also materializes its Run backfill through
`migrationseeds`: it enters Deployment Scope and copies each existing Run's
`updated_at` into its new `created_at` as an approximation. Later Runs receive
their creation time when inserted; `execution/0091_bounded_read_indexes` sets
the physical insert default and creates the ordered, state-filtered indexes.

Reinhardt Query already provides builders for several of these statements; the
remaining gap is their integration into migration operations and filesystem
source loading. Retain the procedural constraints, lease fences, and visibility
barriers when changing a model.

Migration sources and SQL assets use LF line endings. Do not embed NEL, LS, PS,
vertical tab, form feed, or carriage return characters. PostgreSQL escape string
literals preserve the existing Unicode whitespace set in nonblank CHECKs
without putting special line terminators in source files.

The native `FilesystemSource` and `FilesystemRepository` resolve literal
`include_str!` references within confined sibling SQL assets. Execution,
inspection, file-state reconstruction and nonempty `makemigrations` writes read
the original app history directly. Repository reads and save-time duplicate
checks carry the same SQL asset context through source validation and metadata
extraction; the pinned revision incorporates
[#6636](https://github.com/kent8192/reinhardt-web/issues/6636).

For the one-time generation of `0009_desktop_sessions`, the native
`FilesystemSource` loaded all 44 existing records and the native
`FilesystemRepository::render` wrote disposable copies with resolved SQL literals.
The real `manage makemigrations identity --state-source files --name desktop_sessions`
then generated the new file against that temporary directory. Only the new file
was copied into canonical history; all original files retained their SHA256 hashes.
There is no second persisted history or migration engine. New migrations use the
canonical history directly; the temporary resolved-copy generation procedure is
no longer needed. The CLI regression copies the complete history, introduces one
logical field difference, and verifies a nonempty save, unchanged input assets,
the expected dependency, and a subsequent `--dry-run --check` with no changes.

The pinned revision `a068ecbdc03ff01653f80c9c4ab36e15a27f2bd7` includes SQL asset
loading [#6505](https://github.com/kent8192/reinhardt-web/issues/6505) and native
PostgreSQL sequence/identity operations
[#6506](https://github.com/kent8192/reinhardt-web/issues/6506), column-default
restoration, and typed extension reversal
[#6516](https://github.com/kent8192/reinhardt-web/issues/6516). The physical baseline preserves the legacy schema. Its environment migration
creates and comments `pg_jsonschema` only when absent. A preprovisioned extension
retains its owner and comment; reversal drops only an extension carrying this
history's ownership marker. This conditional ownership lifecycle is procedural
DDL because native `CreateExtension { if_not_exists: true }` intentionally cannot
reverse an extension whose ownership is unknown. Subsequent applied migrations must not be
rewritten. New sequence/identity changes should use the native
operations and declare their model metadata; they must not rewrite applied SQL
assets. New extension migrations may use native reversal only with explicit
migration ownership (`if_not_exists: false`); conditional creation cannot establish
ownership for automatic rollback. First-class procedural, session, and seed
operations remain tracked in
[#6507](https://github.com/kent8192/reinhardt-web/issues/6507).
Cross-app reverse planning uses the native command implementation from
[#6515](https://github.com/kent8192/reinhardt-web/issues/6515).

## Commands

From the repository root, apply or inspect the native history:

```sh
cargo run --locked -p aidash-server --bin manage -- migrate
cargo run --locked -p aidash-server --bin manage -- migrate --plan
cargo run --locked -p aidash-server --bin manage -- showmigrations
cargo run --locked -p aidash-server --bin manage -- migrationseeds --check
(cd server && cargo run --locked --bin manage -- makemigrations --state-source files)
(cd server && cargo run --locked --bin manage -- makemigrations --state-source files --dry-run --check)
```

Both migration entry points use the same graph and transaction-scoped advisory
lock `71003203` for the complete run. Initialization cannot use `--fake` or
`--fake-initial`. Run `migrate` as the `aidash_owner` login; it then leaves the
`reinhardt_migrations` ledger read-only to every other role. The compatibility
`serve`, `server`, and `worker` commands never apply the graph: logged in as
`aidash_server`, they read the ledger and refuse to start while any migration is
pending, so a new or upgraded database needs a separate `migrate` run first.

The physical baseline is reversible. Reinhardt's native command expands applied
dependents across app labels, orders them through the complete dependency graph,
and uses its native executor to reverse each migration atomically.
`manage migrate operations zero` reverses the complete baseline, including model
state, functions, triggers, schema objects, seed data, and migration-owned
extension creation. An administrator-provisioned extension is retained, including
when migrations run under a database-scoped role.
Applying the baseline again recreates an empty schema. Partial app targets use
the same dependency graph. `--fake` baseline reversal remains prohibited because
it would remove ledger records while retaining physical objects. `--plan` and
`sqlmigrate --backwards` remain available for inspection. Existing SeaORM history
still cannot be adopted by this native ledger.

For an intentional model change, run `manage makemigrations --state-source files`
from `server/`, where the native command requires `src/bin/manage.rs`. Pass
`--migration-dir /absolute/path/to/migrations` when selecting a different history. Inspect
the generated app migration, and retain any procedural DDL required by the business
invariants. Use one history throughout; do not run the retired SeaORM migrator.
`temporary-db` is inappropriate for this baseline because a generic PostgreSQL
image does not provide its required `pg_jsonschema` extension.

## Tenancy scope settings

Every transaction the server opens carries one tenancy scope (#223, #263),
written right after `BEGIN` by `server/src/database/scope.rs` with
`set_config(..., true)`. Row-level policy functions (#264) and the activity
observer read and write the same strings, so this is a contract across
workspaces:

| Setting                 | Value                                                                                          |
| ----------------------- | ---------------------------------------------------------------------------------------------- |
| `aidash.scope`          | `tenant`, `deployment` or `none` (no Tenant)                                                   |
| `aidash.tenant`         | the Tenant name in a `tenant` scope; empty otherwise                                           |
| `application_name`      | `deployment:<label>` in a `deployment` scope, where `<label>` is the reason's kebab-case label |
| `transaction_read_only` | `on` when the Deployment Scope reason only reads                                               |

- A setting that was never written reads as NULL, and one that a returned
  connection's `RESET ALL` cleared reads as the empty string. Treat both as
  unset: a policy function raises when `aidash.scope` is unset or `none`.
- The scope is fixed for the transaction. Only `database/scope.rs` writes
  `aidash.scope` and `aidash.tenant`; a test rejects any other writer, in Rust
  or in migration SQL.
- Every table is classified in `server/src/database/classification.rs` as
  Tenant-owned (enforced) or Deployment-wide. The `startup` test target compares the list with the
  migrated database, so a migration that adds a table classifies it in the same
  change.
- The Enterprise workspace classifies its own tables in
  `enterprise/server/src/classification.rs`, one `table(...)` line each. The
  Enterprise build hands that list to Community through `Extension::tables`, so
  `Build::tables` returns the combined list that purge reads.
  `enterprise/server/tests/classification.rs` runs the same comparison
  (`server/src/database/classification/comparison.rs`) on a database migrated
  with both workspaces' roots, and its failure names the Enterprise file.

## Row-level policies

Every Tenant-owned table that carries a `tenant` column has one forced policy
(#264, ADR 0040). It is `tenant_isolation`, `PERMISSIVE FOR ALL TO PUBLIC`, with
the same `USING` and `WITH CHECK` expression:

```sql
(SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())
```

`FORCE ROW LEVEL SECURITY` binds the owner too, so `aidash_owner`, the server
login and the activity observer all need a scope.

- `identity/0028_tenancy_policy_functions` creates both functions. They are
  `STABLE` plpgsql with `search_path = pg_catalog`. SQL does not promise that
  `OR` short-circuits, so each one is safe in every scope:

  | `aidash.scope`               | `aidash_deployment_scope()` | `aidash_scope_tenant()` |
  | ---------------------------- | --------------------------- | ----------------------- |
  | `tenant`, with a name        | false                       | the Tenant name         |
  | `tenant`, with an empty name | false                       | raises                  |
  | `deployment`                 | true                        | NULL                    |
  | `none`, unset or unknown     | raises                      | raises                  |

  The raise uses SQLSTATE `TS001`. Its detail names the scope state and never a
  Tenant. The server reports it, like a failed `WITH CHECK`, as an internal
  error, because it means a server bug rather than a refused permission.

- `identity/0028_tenancy_policy_functions` also creates `aidash_tenant_purge()`,
  true only in the `tenant-purge` Deployment Scope (`aidash.scope` is
  `deployment` and `application_name` is `deployment:tenant-purge`), which only
  `server/src/database/scope.rs` can write. Purge deletes rows that triggers
  keep from direct changes, so these `RunSQL` functions let that scope through
  and nothing else: `guard_task_dependencies`, `guard_registry_agent_model_refs`,
  `guard_registry_agent_resource_refs` and `guard_registry_memory_role_refs`
  (rows their parents maintain) and `marketplace_immutable`, for `DELETE` only
  (a Tenant's Marketplace revisions go with the Tenant). Each function calls it
  only after its own check, so it never runs where the function is absent.
  Reinhardt cannot express trigger function bodies (ADR 0061).
- A row whose `tenant` is NULL or empty is visible only in a Deployment Scope.
- Each app's `*_row_level_security` migration enables and forces the policy on
  its own tables and depends on the functions migration: `execution/0036`,
  `federation/0010`, `identity/0029`, `knowledge/0033`, `marketplace/0010` and
  `registry/0033` for the tables that carried a `tenant` column before #265, and
  `NNNN_tenant_owned_row_level_security` (`execution/0043`, `federation/0011`,
  `identity/0032`, `knowledge/0034`, `marketplace/0011`, `registry/0037` and
  `workspaces/0009`) for the tables that gained it there. A table created after
  that carries its own policy migration, such as
  `registry/0040_agent_plugin_copies_row_level_security`. The policy asset test
  selects migrations by the `_row_level_security` suffix, and a separate
  migration after the app's first policy migration keeps those files unchanged
  for the branches that already contain them. Reversal drops the policy,
  removes `FORCE`, then disables row security. Reinhardt has no policy operations
  ([#6770](https://github.com/kent8192/reinhardt-web/issues/6770)), so these are
  `RunSQL` assets. `server/src/database/policy.rs` renders them, and a unit test
  fails when an asset differs from its rendering or when the assets and the
  classification name different tables.
- A policy raises only when a statement scans a row. A migration that writes
  Tenant-owned rows after their policy migration would pass on an empty
  database and fail on a Deployment with data, so no migration may do so. A unit
  test scans every migration source, outside function bodies, for such writes.
  Seed Tenant-owned rows from the server inside a scope, or change the table's
  baseline definition.
- A Tenant-owned Enterprise table gets the same policy from an
  `NNNN_row_level_security` migration of its own app. The policy checks
  (`server/src/database/policy/checks.rs`) run on Community's migrations in the
  unit tests and on the Enterprise root in
  `enterprise/server/tests/classification.rs`, where they also refuse a write
  to any Tenant-owned row of either workspace.
- Test fixtures that seed or inspect Tenant-owned rows with plain SQLx use
  `Pool::fixture_driver()`. Its sessions hold the `test-fixture` Deployment
  Scope, and scoped transactions still override it. Only debug builds have it,
  and clippy bans it outside tests.

## The Tenant on every Tenant-owned table

Every Tenant-owned table carries `tenant text NOT NULL` (#265), except
`research_revocations` and `dashboard_administration_history`, whose rows may
address no Tenant. A row without a Tenant is visible only in a Deployment
Scope. The definitions are edited in the
migration that creates each table, because Aidash is undeployed:

- `tenant` references the Tenant root record, `tenants(name)`, through
  `<table>_tenant_root_fkey`. The root record outlives a purge as a tombstone.
  `tenants` is therefore created with the other identity tables in
  `identity/0002_tables` rather than in `identity/0020`: the baseline tables of
  every app precede `identity/0020`, and a reference needs its target first.
- A child keeps its parent's Tenant through a composite key,
  `(tenant, parent_id) -> parent(tenant, id)`. Each parent gains
  `UNIQUE (tenant, <referenced columns>)` named `<table>_tenant_<columns>_key`
  for it. Every reference between two Tenant-owned tables compares `tenant`;
  a test in `server/src/apps/execution/tests/tenancy.rs` fails on one that does
  not. A reference whose parent is on another Node, or has no foreign key,
  carries the local Tenant and the `tenants` reference only.
- A table created by a state-and-database migration (`knowledge/0009`, for
  example) lists these constraints inline, and the following state-only
  migration drops them from the model state again, as it does for the table's
  other references.
- Trigger functions that insert into these tables copy `NEW.tenant`
  (`execution/sql/forward/0001_functions.sql`, `workspaces/sql/forward/0001_functions.sql`).
- `marketplace_requests` is keyed by `(tenant, key)`. `marketplace_consents`
  stays Deployment-wide: a source's owner writes a consent for the redistributor
  that reads it, so it belongs to neither Tenant's scope, as
  `marketplace_audiences` and `marketplace_versions` do not.
- A Node is a Tenant-owned row in `nodes`, keyed by `(tenant, name)`, and the
  Node-owned tables reference `nodes(tenant, name)` with `(tenant, node)`
  (#241). `node_names` is Deployment-wide and its rows are never deleted: it
  keeps a Node name taken after a purge, and it names the Tenant of a receiving
  Node before any Tenant scope opens. `nodes` references it with
  `(name, tenant)`. Both are created in `identity/0002_tables` after `tenants`.
- Node-owned data carries `node` (#241), edited into the migration that creates
  each table: `workspaces.node`, `runs.node`, `generation_requests.node` and
  `authorization_catalog` plus its history, each with `(tenant, node)`
  referencing `nodes(tenant, name)`. `events.node` is the Workspace's Node and
  stays NULL for an event that concerns no Workspace. `authorization_catalog`
  and its history key on `(tenant, node, entry_id, entry_version[, revision])`,
  so one Node's approval never stands for another's. `runs.home_node` and
  `generation_requests.home_node` hold the address of the Home Node; a local
  Run's is its own Node's. `generation_requests.local_task_id` and
  `local_workspace_id` are ordinary columns the application writes for a
  local request and leaves NULL for a foreign one; the `''` sentinel and the
  generated columns are gone. `memory_learning_discovery` is keyed by
  `deployment`, because the learning pass runs once per Deployment.
- A Node Registration is a Tenant-owned row in `federation/node_registrations`,
  keyed by `(tenant, node, source_node)`, with `(tenant, node)` referencing
  `nodes` (#241). The registered Node is an address, and a Node of another
  Deployment pins that Deployment's endpoint, public key and key ID in three
  columns that are all set or all null. `node_registration_history` keeps every
  revision under a sequence, like `authorization_peer_mapping_history`. Both
  tables are purged with the Tenant. They are created in
  `federation/0002_tables` and enforced by `federation/0011`.
- Inserts name `tenant`. Statements built for a Tenant scope write the scope's
  Tenant with `crate::database::scope::tenant()`, a child of a row reads its
  parent's with `tenant_of`, and a writer in a Deployment Scope names the Tenant
  itself.

## Nodes and Federation migrations

Aidash is undeployed, so #241 edits each migration below in place and adds no migration file. Each paragraph names what the edited migration now creates, drops or renames. The bullets above describe the same tables by policy and key.

`identity/0002_tables`, `identity/0003_keys_indexes`, `identity/0004_references` and `identity/0007_model_state` create `node_names` and `nodes` after `tenants`. `authorization_catalog` and its history gain `node`, and their primary keys start with `(tenant, node)`.

`workspaces/0002_tables`, `workspaces/0004_references` and `workspaces/0007_model_state` add `workspaces.node`, which references `nodes(tenant, name)`.

`execution/0002_tables`, `execution/0003_keys_indexes`, `execution/0004_references` and `execution/0007_model_state` add `runs.node`, `generation_requests.node` and `events.node`. `events.node` stays NULL for an event that concerns no Workspace. `events.tenant` becomes NOT NULL, because the `peer.registered` event, the only event without a Tenant, goes with `peers`. The `''` sentinel in `generation_requests.home_node` and its two generated columns are gone. `local_task_id` and `local_workspace_id` are ordinary nullable columns that the application writes. `generation_requests_home_task` is unique on `(home_node, task_id)` for an active foreign request, and `generation_requests_local_task` is unique on `local_task_id`.

`federation/0002_tables`, `federation/0003_keys_indexes`, `federation/0004_references` and `federation/0007_model_state` remove `peers`, `peer_events` and `atomic_peer_trust`, with their foreign keys. They create `node_registrations` and `node_registration_history`, whose sequence is `node_registration_history_sequence_seq`. `authorization_peer_mappings` and its history lose `source_tenant`, gain `node`, and key on `(tenant, node, source_node, source_subject)`. `authorization_remote_admissions` and `authorization_remote_grants` gain `node`.

`federation/sql/forward/0001_functions.sql`, `0002_configuration.sql`, `0002_sequences.sql`, `0006_triggers.sql`, `0011_tenant_owned_row_level_security.sql` and their backward assets replace `peers` with `node_registrations` and `node_registration_history` in the Tenant-owned table list. The `atomic_write_guard` statement trigger moves from `peers` and `peer_events` to the two new tables. The sequence for `node_registration_history` is created, and `0011` enables and forces row-level security on both new tables with the `tenant_isolation` policy.

`identity/sql/forward/0032_tenant_owned_row_level_security.sql` and its backward asset enable and force row-level security on `nodes`. `node_names` has no policy, because it is Deployment-wide.

`knowledge/0018_memory_control_receipts` keys `memory_learning_discovery` on `deployment` in place of `home`, because the learning pass runs once per Deployment.

The forward and backward SQL assets from `registry/0008_provider_descriptors` to `registry/0042_external_services` rename the JSON key `registry_node` to `registry_deployment`. The `aidash://` checks keep their pattern.

## Maintenance cutover

1. Stop new writes and request graceful shutdown of every old server and worker.
   Wait for drain, then back up their database and retain the old binaries.
2. Provision a separate empty PostgreSQL 17 database with the `pg_jsonschema` 0.3.4
   library available. As a superuser, create the `aidash_owner` and
   `aidash_server` logins, make `aidash_owner` own the database, create the
   `pg_jsonschema`, `vector` and `pgroonga` extensions, and grant `aidash_server`
   default privileges on the owner's tables and sequences, as
   `deploy/postgres/init-roles.sh` does. Neither login is a superuser or has
   `BYPASSRLS`. Configure all new replicas to use that database, the
   `aidash_server` login and the same pinned image; servers and workers refuse
   to start as a superuser, a `BYPASSRLS` role or an owner of database objects.
   For a non-Compose database, run `aidash setup-roles --superuser-url <url>`
   once to create the roles, database ownership, default privileges and
   extensions. Save
   the printed URLs securely; existing roles retain their passwords unless
   `--owner-password` or `--server-password` is supplied. The PostgreSQL image
   or package must provide the extensions; `local-dev-db` and `migrate` check
   for `pg_jsonschema` 0.3.4.
3. Run the native migration command once as `aidash_owner`. Inspect `showmigrations` and verify
   replay, schema constraints, and configured Federation and identity settings before
   accepting writes.
4. Start the new server and worker roles. Verify health, authentication, a scoped
   workflow, SSE reconnection, and the configured Node Registrations.

This cutover starts a new database; it does not transfer existing workspaces,
identities, credentials, or active Runs. Keep the old deployment/database intact
for rollback. Stop all new writers before restoring the old configuration.
Never let old and new migration engines manage the same application schema.

## Physical schema verification

`test-migration-schema.py` compares the shared migration bootstrap on a unique empty
`template0` database with a read-only PostgreSQL reference built from the immutable
native revision `cd29635a9937133d2e81cca44857bea331446cc5`. This reference includes
native memory, vector, and PGroonga additions. Prepare it with `aidash migrate`
using binaries, settings, and migration sources from that exact revision in an
isolated deployment directory; do not rebuild the reference from the working tree.
The frozen reference identities are checked into
`scripts/migration-schema-reference.json`; verification does not need the original
Git object, so shallow clones and squash merges retain the reference ledger.
The reference must match that manifest, and the target must have exactly the
`(app, name)` identities registered in its current sources, with no retired ledger.
New migration files are included automatically without a hard-coded record count.

```bash
python3 scripts/test-migration-schema.py \
  --aidash /absolute/path/to/aidash \
  --manage /absolute/path/to/manage \
  --postgres-container aidash-schema-reference \
  --postgres-port 54370 \
  --reference-database aidash_reference
```

The verifier repeats `manage migrate` and runs
`makemigrations --state-source files --dry-run --check`. It compares all public
tables, columns, constraints, indexes, triggers, functions, aggregates, sequences, and the
`pg_jsonschema`, `vector`, and `pgroonga` extensions. Names, types, defaults,
generated columns, key definitions, deferred/validated flags, index predicates,
trigger enablement/function bodies, and sequence ownership/settings must match
verbatim. Counts alone never establish parity. The script removes only its own
target database and writes complete catalogs, ledgers, command exit codes, source
state, and executable hashes under `private/schema-parity/`. Intentional physical
schema changes require a separately reviewed immutable reference update.

The native memory additions end with generated state-only ORM metadata checkpoints
for execution, knowledge, and registry. These reconcile composite-key and field
metadata and exclude procedural checks and references from the ORM snapshot, while
retaining the physical constraints established by the preceding migrations.

Binding and native memory histories converge in `registry/0015_binding_memory_merge` and `execution/0011_binding_memory_merge`. The registry merge preserves strict Agent Bindings, Host lifecycle validation and native memory operations. Agent memory-role references derive from qualified Binding targets. These migrations depend on both histories and leave their existing migration identities unchanged.

`registry/0018_prompt_cache` admits the Agent `prompt_cache` key (`off` or `explicit`) in the Agent Binding contract and the model `cache_mode` key in the `registry_model_config` allowlist. It builds on the `registry/0017_projection_versions` Agent contract and edits the current model constraint read from the catalog (`pg_get_constraintdef`), so earlier additions such as `provider_credential` and `projection_versions` survive; the edit raises if its allowlist anchor is missing. The separate `registry_model_cache_mode` constraint limits `cache_mode` to `none`, `automatic` or `explicit`. The database does not enforce the explicit-caching slug allowlist; Registry validation does. Function bodies and catalog-driven constraint edits have no typed Reinhardt operation, so the migration uses reversible SQL assets. The reverse migration restores the 0017 Agent contract and removes only the `cache_mode` additions. Its precondition is that no registry Definition or Agent Package carries either key: pre-0018 binaries reject such configs and registered Definitions are immutable, so a read-only guard refuses the rollback with SQLSTATE `55000`. Restore the pre-upgrade database backup instead.

`registry/0019_deferred_exposure` admits the Agent `exposure` key (`legacy@1` or `deferred@1` with its byte budgets), Binding `exposure` (`eager` or `deferred`) under `deferred@1`, and the `core.exposure@1` and `skill_asset_read` builtin descriptors. It builds on the `registry/0018_prompt_cache` Agent contract, keeping its `projection_version` and `prompt_cache` keys, and adds `exposure` to the current installation guard's Agent override allowlist read from `pg_get_functiondef`; the edit raises if its anchor is missing. Rollback restores the `0018_prompt_cache` function bodies and withdraws the override. Function bodies and catalog-driven edits have no typed Reinhardt operation, so they live in SQL assets.

`registry/0020_model_streaming_config` admits the model `streaming` and `stream_stall_timeout_secs` keys. It edits the current `registry_model_config` allowlist read from `pg_get_constraintdef` and the model section of the current installation guard read from `pg_get_functiondef`, so earlier additions such as `cache_mode` and the `registry/0019_deferred_exposure` Agent `exposure` override survive; each edit raises if its anchor is missing. The separate `registry_model_streaming` constraint validates the types. Rollback refuses while any model Definition, Package or installation override uses the keys, then withdraws exactly those edits. Catalog-driven edits have no typed Reinhardt operation, so they live in SQL assets.

`registry/0021_tool_parallelism` (after `registry/0020_model_streaming_config`) replaces the Agent Binding and descriptor contract functions. They now accept the Agent `tool_parallelism` ceiling (1–16) and a descriptor, Tool Binding or Bundle Binding `narrow.concurrency` restriction (`sequential` or `shared_read`), and keep the `projection_version`, `prompt_cache` and `exposure` fields of `registry/0019_deferred_exposure`. It adds `tool_parallelism` to the current installation guard's Agent override allowlist read from `pg_get_functiondef`; the edit raises if its anchor is missing, and leaves the `0020_model_streaming_config` model section intact. Existing rows are unchanged; the reverse migration refuses to run while any Definition, Package or installation declares `tool_parallelism` or `narrow.concurrency`, or while an unfinished Run's Binding snapshot pins a shared-read core operation (`file_read`, `file_search`, `skill_list`, `skill_load`) or a concurrency restriction, because those provider contract digests encode `concurrency` and earlier binaries reject them. It then restores the `0019_deferred_exposure` Agent Binding and descriptor bodies and withdraws the override.

`registry/0022_context_policy` (after `registry/0021_tool_parallelism`) replaces the Agent Binding contract function so it accepts the Agent `context_policy` key with the constraints of `ContextPolicy::validate`: version `context-recovery/1`, `preserve_recent` 6–1000, `overflow_retries` 0–4, `overflow_shrink` 0.5–0.9, and an optional `summary` naming an exact model reference with `max_tokens` 256–4096 and `call_budget` 1–64; unknown keys are rejected. It keeps the 0021 body otherwise, including `tool_parallelism`, `exposure`, `prompt_cache` and `projection_version`. Context Policy is not an installation override, so the installation guard is unchanged. Existing rows are unchanged; the reverse migration refuses to run while any Definition, Package or installation declares `context_policy`, or while an unfinished Run's Binding snapshot pins an Agent definition declaring one, and otherwise restores the `0021_tool_parallelism` body. The function body has no typed Reinhardt operation, so it lives in SQL assets.

`execution/0012_inference_usage` creates the per-attempt Usage Record table with typed operations, including its Run reference, outcome and count checks, `created_at` default and `(run_id, outcome)` lookup index. The generated state-only checkpoint `execution/0013_inference_usage_model_state` removes those from the ORM snapshot without dropping them physically. `projection_version` is `integer` because the pinned ORM model macro has no Rust mapping for `smallint`.

`execution/0016_context_journal` follows `execution/0015_inference_progress_state`
and creates the append-only `run_context_events`
Context Journal and `context_compaction_attempts` with typed operations: composite
`(run_id, seq)` key, cascading Run references, origin/stage/outcome CHECKs, a
`(run_id, stage)` lookup index and a partial unique index allowing one unsettled
attempt per Run. Only the two `atomic_write_guard` statement triggers use
`RunSQL`, because the typed API cannot express trigger DDL. Like the desktop
schema, this physical migration is database-only and precedes its state-only
model snapshot `execution/0017_context_journal_model_state`. `runs.context`
remains the lossy projection; stored projections written before the journal are
upgraded when decoded, and their first save, or the worker before their first
pruning or summary, journals those events as `imported`.

`execution/0020_research_evidence` (after `execution/0019_summary_allowance_state`) creates the Run-scoped Research evidence tables `research_attempts`, `research_sources`, `research_snapshots` and `research_fragments` with typed operations. Snapshots and fragments reference their parent through `(run_id, id)` composite keys, so evidence can never point at another Run's snapshot; every table cascades with its Run. `research_attempts` is unique per `(run_id, invocation_key)`, records the Search account holder's account and price together or not at all, and has a partial `(account_id, created_at)` index for monthly limits. The generated state-only checkpoint `execution/0021_research_evidence_model_state` removes the checks, references, defaults and index from the ORM snapshot without dropping them physically.

`execution/0025_decision_evidence` (after `execution/0024_estimate_calibration_state`) creates the Decision Evidence tables `decision_attempts`, `decision_evidence`, `decision_states` and `decision_outcomes` with typed operations. Every table cascades with its Run and records the Decider by its exact pin as JSONB, never through a Registry reference. `decision_attempts` is keyed by the stable attempt identity and keeps the reserving worker and Run revision; its CHECKs tie `pending` to a missing `finished_at` and `answered` to stored answers. A partial unique index allows one `applied` or `shadow` snapshot per decision, so only rejected snapshots repeat. Retained state references its evidence through `(run_id, evidence_id)`, so it can never point at another Run's evidence. Only the four `atomic_write_guard` statement triggers use `RunSQL`, because the typed API cannot express trigger DDL. Like `execution/0016_context_journal`, this physical migration is database-only and precedes its generated state-only snapshot `execution/0026_decision_evidence_model_state`.

`execution/0027_url_intents` (after `execution/0026_decision_evidence_model_state`) creates `url_intents`: the pages a person asked one Run to read, recorded beside the input that carried them instead of in its text (ADR 0052). A row without `input_key` belongs to the Run's Task; any other names a run input's idempotency key. It cascades with its Run, bounds `pages` to one to five entries and has a `run_id` lookup index. The generated state-only checkpoint `execution/0028_url_intents_model_state` removes the check, reference, default and index from the ORM snapshot without dropping them physically.

`registry/0023_research_providers` (after `registry/0022_context_policy`) admits the research contract Node providers (`research.brave@1`, `research.page@1` and `research.fixture@1` with their fixed operations and tiers, no transport and no lifecycle) in `aidash_descriptor_is_valid`. The function body has no typed Reinhardt operation, so it is a reversible SQL asset. It extends the `0021_tool_parallelism` body, keeping its concurrency narrowing rules, and rollback restores that body. It does not delete research descriptors already registered, whose later rewrites the NOT VALID registry checks then reject, so remove research packages before rolling back.

`registry/0028_tool_transport_secrets` (after `registry/0027_remove_legacy_installations_state`) removes `credential_env` from the HTTP and MCP transports accepted by `aidash_tool_config_is_valid` (#296): a Tool never names a Deployment secret. The function body has no typed Reinhardt operation, so it is a reversible SQL asset, and rollback restores the `registry/0001` body. The registry checks are NOT VALID, so a Tool stored before this migration keeps its row; the server refuses its transport when the Tool runs.

`registry/0030_plan_connection_models` (after `registry/0029_completion_policy`; like it and `registry/0028_tool_transport_secrets`, it follows the Tenant Registry migrations of #243) admits Plan Connection models (#159): it adds `plan_connection` to the current `registry_model_config` allowlist and widens its provider to `openrouter` or `chatgpt`, both read from `pg_get_constraintdef` and raising if an anchor is missing. The separate `registry_plan_connection_config` check makes a `chatgpt` model name `plan_connection = "chatgpt"` and OpenAI's endpoint without a Provider Credential, and keeps every other model from naming a Plan Connection. Rollback refuses while any model Definition uses a Plan Connection, then withdraws exactly those edits. Catalog-driven edits have no typed Reinhardt operation, so they live in SQL assets. `identity/0022_plan_connections` (after `identity/0021_tenancy_state`) and `identity/0023_plan_connection_constraints` create the Plan Connection tables and their checks, keys and cascading Run pins with typed operations.

`registry/0031_model_request_estimator` (after `registry/0030_plan_connection_models`, and like it after the Tenant Registry migrations of #243) admits the model `request_estimator` key of #179 (ADR 0017), which the dashboard sends as `calibrated` for every new model version (#367). It adds the key to the current `registry_model_config` allowlist read from `pg_get_constraintdef`, raising if the anchor is missing. The separate `registry_model_request_estimator` check limits the value to `bytes` or `calibrated`. Rollback refuses with SQLSTATE `55000` while any model Definition carries the key, then withdraws exactly those edits. Catalog-driven edits have no typed Reinhardt operation, so they live in SQL assets.

`registry/0032_degraded_context` (after `registry/0031_model_request_estimator`) admits the `workspace_retrieval` source key `on_unavailable: pause | degrade` of #234 (ADR 0059) by replacing `aidash_context_is_valid`; no other adapter may carry it. Rollback refuses with SQLSTATE `55000` while any source Definition or Marketplace package declares the key (the package count is skipped when `packages` lives in another database), then restores the `registry/0011_binding_contract` validator. A PostgreSQL function body has no typed Reinhardt operation, so it lives in SQL assets.

`registry/0034_research_view` (after `registry/0033_row_level_security`, and like `registry/0031_model_request_estimator` after the Tenant Registry migrations of #243) admits the research `view` operation for `research.page@1` and `research.fixture@1` in `aidash_descriptor_is_valid` (#312). The function body has no typed Reinhardt operation, so it is a reversible SQL asset; it extends the `0023_research_providers` body and rollback restores that body. It does not delete descriptors that already declare `view`, whose later rewrites the NOT VALID registry checks then reject.

Before this native history is released, the same current function also admits the exact `core.preview@1` Sandboxed descriptors for `live_preview_start` and `live_preview_stop` (#405). Neither operation has a transport or lifecycle; all other providers, tiers, and descriptor fields remain subject to the existing checks.

`registry/0035_decider_kind` (after `registry/0034_research_view`, and like `registry/0031_model_request_estimator` after the Tenant Registry migrations of #243) removes the `compactor` kind (#108 Part B, ADR 0042): it drops `registry_compactor_config` and replaces `registry_kind_check` with a list that names `decider` instead. A Decider's configuration is validated by the application. Aidash is undeployed, so there is no data rewrite: the new check fails while a compactor entry exists, and such entries must be registered again as Deciders. Typed constraint operations suffice. The generated state-only checkpoint `registry/0036_decider_kind_state` swaps `compactor` for `decider` in `registry_kind_model_enum_check`.

`registry/0038_agent_plugin_copies` (after `registry/0037_tenant_owned_row_level_security`) creates `agent_plugin_copies` with typed operations (#414, ADR 0072): the Tenant's copy of each imported Agent Plugin, keyed by `(tenant, digest)` where `digest` is the plugin's tree digest. A row keeps the serialized loaded plugin (`plugin`), every fetched file (`files`, text or base64) and the paths the Agent Plugins specification forbids (`denied`). Checks require the `sha256:` digest form, the digest in `plugin` to equal the key and the three documents to have their JSON types. The copy exists before the Agent version that pins it, so the table has no key into `registry`; it references the Tenant root record through `agent_plugin_copies_tenant_root_fkey`. Rows are immutable: the repository inserts with `ON CONFLICT DO NOTHING` and never updates. `registry/0039_agent_plugin_copies_state` is the generated state-only checkpoint.

`registry/0040_agent_plugin_copies_row_level_security` (after `registry/0039_agent_plugin_copies_state` and `identity/0028_tenancy_policy_functions`) enforces the forced Tenant policy on `agent_plugin_copies`, as `registry/0037` does for the tables that gained a `tenant` column in #265. It is a database-only `RunSQL` migration whose assets are the policy template rendered for the one table.

`registry/0041_agent_plugins_definition` (after `registry/0040_agent_plugin_copies_row_level_security`) replaces the Agent Binding contract function so it accepts the Agent `agent_plugins` key of #414: an array of at most 8 objects, each with a `sha256:` `digest`, string `name` and `source`, `skills` and `findings` arrays and a `servers` object. An explicit null is rejected, and a plugin that carries Skills lets the Agent's instructions be empty, as a Skill Binding does. `AgentBindings::validate` checks the rest. It keeps the `0029_completion_policy` body otherwise. Rollback refuses with SQLSTATE `55000` while any Agent definition declares `agent_plugins` or any Run pins an Agent definition that does, then restores the `0029_completion_policy` body. The function body has no typed Reinhardt operation, so it lives in SQL assets.

`registry/0045_agent_plugin_settings` (after `registry/0044_foreign_key_relations_state`) replaces the Agent Binding contract function so it accepts the Agent `agent_plugin_settings` key of #414: what an Agent version decides about the servers of the plugins it takes in (ADR 0072). The key is an object whose members are objects, plugin name to server name to settings. A settings object holds only `tools` (an array of strings, or null), `exposure` (`eager` or `deferred`, or null) and `read_only` (an array of objects with a string `name` and a string `schema_digest`). An explicit null is rejected for the key and for `read_only`, whose Rust fields have no `Option`. `AgentBindings::validate` checks the rest: that each key names an attached plugin and server, the name and count limits, and the digest format. It keeps the `0042_external_services` body otherwise, including External service Bindings. Rollback refuses with SQLSTATE `55000` while any Agent definition declares `agent_plugin_settings` or any Run pins an Agent definition that does, then restores the `0042_external_services` body. The function body has no typed Reinhardt operation, so it lives in SQL assets.

`workspaces/0008_publishing_run` (after `workspaces/0007_model_state`) adds a nullable `run_id` to `messages` and `artifacts` with typed `AddColumn` operations. It records the Run that published the text on this Node, so a Source citation in it resolves only against that Run's Research evidence. It has no foreign key: the value outlives a deleted Run, so such a citation reads as unavailable rather than as text nobody published. Existing rows keep `NULL`, and rollback drops both columns.

`execution/0029_research_retention` (after `execution/0028_url_intents_model_state`) adds `purged_at` and `purge_reason` to `research_snapshots` with typed operations. A check keeps both set or both null, limits the reason to `retention` or `revocation`, and requires a Purged snapshot to keep no contents (`lines = '[]'`, `text_bytes = 0`). A partial index on unpurged snapshot ids serves the retention sweep. It also creates `research_revocations`, keyed by Run with no reference to `runs`, so the audit record of an Evidence Revocation outlives its Run (ADR 0046), with a `(tenant, revoked_at)` index for listing. The generated state-only checkpoint `execution/0030_research_retention_state` records the columns and table in the ORM snapshot without the checks and indexes.

`execution/0031_research_originals` (after `execution/0030_research_retention_state`) records, for a snapshot of a paginated source, its page count, the pages that carry no text, and the digest and size of its original, with typed `AddColumn` operations (#312, ADR 0053). A check keeps the digest and size set together, bounds the size to 16 MiB and allows `original_retained` only while the snapshot is not purged. It creates `research_originals`, keyed by snapshot with a cascading reference to `(run_id, id)` of `research_snapshots`, so that reading a snapshot never loads its original; a purge or an Evidence Revocation deletes the rows, and a `run_id` index serves the per-Run limit. The generated state-only checkpoint `execution/0032_research_originals_state` records the columns and table without the checks and index.

`execution/0033_research_event_run_index` (after `execution/0032_research_originals_state`) adds the partial expression index `events_tool_run` on `(data ->> 'run_id')` for `tool.started` and `tool.completed` events, which Evidence Revocation and the retention sweep redact per Run. It changes no model state, so it has no state-only checkpoint.

`execution/0034_tokenized_estimate_confidence` (after `execution/0033_research_event_run_index`) replaces the `inference_usage_estimate_confidence` check with typed constraint operations, so that a Usage Record may carry the `tokenized` confidence of a Token Encoding (#276, ADR 0017) and no longer the unused `exact`. The check has been enforced apart from the ORM state since `execution/0013_inference_usage_model_state`, so the migration is database-only and has no state-only checkpoint.

`execution/0035_run_semantic_retry_cycle` (after `execution/0034_tokenized_estimate_confidence`) adds `runs.semantic_retry_cycle`, an integer that defaults to 0, with a typed `AddColumn` (#291, ADR 0057). It holds the Run's Retry Cycle: an authorized manual retry of a semantic pause increments it, and it never decreases. `knowledge/0032_memory_attempt_retry_cycle` (after `knowledge/0031_embedding_provider_keys`) adds `memory_model_attempts.cycle`, which also defaults to 0, so a native-memory retry ledger counts only the attempts of the current cycle. Existing Runs and attempts belong to cycle 0. Both columns are in the model state, so neither migration needs a state-only checkpoint, and rollback drops both columns.

`execution/0037_research_page_images` (after `execution/0036_row_level_security`) adds the page image of an Evidence fragment (`image_page`, `image_media_type`, `image_width`, `image_height`, `image_digest`) to `research_fragments` with typed operations (#312, ADR 0053). It replaces the `research_fragments_lines` check so that a fragment is either a passage (no image columns, `line_start >= 1`) or one PNG page image of at most 1568 pixels per edge with no lines. It creates `research_page_images`, keyed by fragment with a cascading reference to `research_fragments`, at most 1 MiB per image, so that reading fragments never loads image bytes; an Evidence Revocation deletes the rows, and a `run_id` index serves the per-Run view limit. The generated state-only checkpoint `execution/0038_research_page_images_state` records the columns and table without the checks and index.

`execution/0041_decision_evidence_v3` (after `execution/0040_research_account_terms_state`) moves Decision Evidence to version 3 (#108 Part B, ADR 0056) with typed operations. `decision_attempts` gains `question_digest`, the commitment to the definitions of the questions an attempt asked, and its shape check requires the digest form. `decision_evidence` admits the `staged` outcome and the `incomplete_answers` reason, and the partial unique index `decision_evidence_final` now keeps one `applied`, `staged` or `shadow` snapshot per decision. Aidash is undeployed, so version 2 rows are not migrated: the new column has no default, and the migration fails while a Decision Attempt exists. The generated state-only checkpoint `execution/0042_decision_evidence_v3_state` records the column in the ORM snapshot without the checks.

`execution/0044_decision_routes` (after `execution/0043_tenant_owned_row_level_security`) moves Decision Evidence to version 4 for Decider Routes (#153, ADR 0066) with typed operations. `decision_attempts` gains `retry`, the step's outage retry, and `route`, the Decider Route the attempt asked; a decision identity commits to both, so each Route and each retry is a decision of its own. The shape check requires both to be non-negative. Aidash is undeployed, so version 3 rows are not migrated: the new columns have no default, and the migration fails while a Decision Attempt exists. The generated state-only checkpoint `execution/0045_decision_routes_state` records the columns in the ORM snapshot without the checks.

`identity/0020_tenancy_schema` (after `identity/0019_provider_authorizations`) creates the Tenant root records (`tenants`, ADR 0050), `personal_tenants` and `tenant_invitations` (indexed by Tenant with `expires_at` and with `created_at`) with typed operations, adds the addressing `tenant` and `invitation_id` columns to `dashboard_registration_requests` (indexed with `status` and `expires_at` for one Tenant's pending requests) and `invitation_id` to `dashboard_administration_history`, and references `authorization_bundles.tenant` to its root record. A reversible SQL asset backfills a root record for every existing policy bundle, and a Personal Tenant link for every `personal-<identity>` Tenant, before the reference is added. Making `dashboard_administration_history.identity_id` nullable is the one `ALTER TABLE` statement in `RunSQL`: the pinned typed `AlterColumn` renders no `NOT NULL` change for PostgreSQL ([#6803](https://github.com/kent8192/reinhardt-web/issues/6803)). The state-only checkpoint `identity/0021_tenancy_state` records the matching models.

`identity/0024_tenant_retention_rules` (after `identity/0023_plan_connection_constraints`) creates `tenant_retention_rules`, one row per Tenant that keeps its own snapshot periods, and `tenant_retention_rule_history`, one row per change, where a cleared change has no periods (ADR 0045). Checks bound every period to ten years. The tables carry a `tenant` column; their row-level policies come with RLS step 3 (#264). The generated state-only checkpoint `identity/0025_tenant_retention_rules_state` records both tables in the ORM snapshot without the checks and index.

`identity/0026_plan_connection_identity` (after `identity/0025_tenant_retention_rules_state`) adds `plan_connections.identity_id`, the External Identity that connected a Plan Connection (#315), and `run_plan_connections.identity_id`, the identity a Run's pin was admitted for. A connection serves calls only while that identity is the only active one mapped to its subject, and is retired once that identity's Mapping is disabled; a pin serves only while the connection still names its identity, so a reconnection by another identity ends the Runs admitted before it. Both columns are `NOT NULL` without a default or backfill: no Deployment holds Plan Connections yet, so a database that already has one must disconnect it, which deletes its pins, before migrating. The database-only `identity/0027_plan_connection_identity_reference` references both to `dashboard_identities`, like every other `identity_id` column, with no cascade: identities are never deleted.

`identity/0028_tenancy_policy_functions` (after `identity/0027_plan_connection_identity_reference`) creates the row-level policy functions `aidash_deployment_scope()` and `aidash_scope_tenant()` (#264). `execution/0036_row_level_security` (after `execution/0035_run_semantic_retry_cycle`), `federation/0010_row_level_security` (after `federation/0009_pinned_delegations_state`), `identity/0029_row_level_security`, `knowledge/0033_row_level_security` (after `knowledge/0032_memory_attempt_retry_cycle`), `marketplace/0010_row_level_security` (after `marketplace/0009_remove_compatibility_gate_state`) and `registry/0033_row_level_security` (after `registry/0032_degraded_context`) each also depend on it and enforce the forced policy on their app's Tenant-owned tables; see Row-level policies above. All seven are database-only `RunSQL` migrations with reversible SQL assets.

`identity/0030_tenant_deletions` (after `identity/0029_row_level_security`) creates `tenant_deletions`, one Deployment-wide row per recorded Tenant Deletion (ADR 0060), referencing the Tenant's root record and the acting External Identities. A check ties each actor kind to its identity and forbids a purged deletion from being restored. A partial unique index keeps at most one unrestored deletion per Tenant, which every authenticated boundary checks, and a second partial index orders the deletions awaiting purge by their deadline. The generated state-only checkpoint `identity/0031_tenant_deletions_state` records the model without the checks and indexes. The same migration holds the purge state (ADR 0061): `purge_step` (the step that runs next or keeps failing), `purge_failures` and `purge_error` with `purge_failed_at` (what the failed step reported), `purge_lease` and `purge_leased_until` (the worker that owns the purge, and until when; every destructive step locks the row and checks it still holds the lease, and a restore is refused while one does) and `audit_erased_at` (when the audit history that outlived the purge was deleted). The checks keep the step a known one, the error set whenever failures are counted, and a purged deletion free of progress, and a third partial index serves the sweep that deletes audit history. `identity/0004_references` creates neither `authorization_revisions_tenant_fkey` nor the foreign key from `authorization_catalog_history` to `authorization_catalog`: audit history is kept after purge and must not reference data purge deletes, so both reference only their Tenant's root record.

`identity_approval_preferences`, `identity_approval_rules` and `identity_thread_approval_modes` hold a person's own Approval Mode and Approval Rules (#411, ADR 0076). Aidash is undeployed, so they are edited into the baseline, as the Tenant column was: tables in `identity/0002_tables`, keys and the unique rule index in `identity/0003_keys_indexes`, the `<table>_tenant_root_fkey` references in `identity/0004_references`, the ORM snapshot in `identity/0007_model_state`, and the row-level policies in the `identity/0029_row_level_security` assets. `identity_approval_preferences` is keyed by `(tenant, subject)` and keeps the default mode, the Answer Deadline (#413) and when the person accepted the risk of Bypass; a check refuses `bypass` as the default before that. `identity_approval_rules` is keyed by `(tenant, id)`; its unique index on `(tenant, subject, kind, effect, target)` also serves the per-approval read of one owner's rules and refuses a rule stored twice. `identity_thread_approval_modes` is keyed by `(tenant, thread_id, subject)` and has no reference to the workspaces app: the endpoint checks that the Thread exists. A development database made before this change is recreated.

`identity_tenant_approval_settings` holds the approval Decider a Tenant chose for the Auto Approval Mode (#412, ADR 0077), keyed by `(tenant)`. A Tenant without a row uses the built-in `aidash.approval-decider`; a row whose `decider_id` and `decider_version` are both null (a check ties them together) is a Tenant that reset to it, and its `revision` keeps counting. It is edited into the baseline for the reason the approval tables above were: `identity/0002_tables`, `identity/0003_keys_indexes`, `identity/0004_references` (`identity_tenant_approval_settings_tenant_root_fkey`), the ORM snapshot in `identity/0007_model_state` and the row-level policy in the `identity/0029_row_level_security` assets. The same change widens two checks of `execution/0041_decision_evidence_v3`: `decision_evidence_outcome` also accepts the approval outcomes `allowed` and `blocked` and the reasons `clear`, `data_exfiltration`, `destructive`, `beyond_request`, `injected`, `boundary` and `unavailable`, and `decision_evidence_final` also makes an `allowed` or `blocked` decision final. An approval decision is keyed by its evidence identity, which commits to the approval, the Decider, the Route and the state, so a retry under another worker finds the evidence it already recorded. A development database made before this change is recreated.

`identity/0033_demo_data` (after `identity/0032_tenant_owned_row_level_security`) creates the three tables of Demo data (ADR 0079, ADR 0080) with typed operations. `demo_data_loads` is Deployment-wide: one row per pair of `demo-data` inputs that loaded to completion, keyed by the lowercase hex SHA-256 of their canonical JSON, written last in its own transaction so a rerun with the same digest changes nothing. `demo_tenants` is Deployment-wide and names a Tenant (purge deletes it with the Tenant): it maps a structure key (`[a-z][a-z0-9-]{0,31}`) to the Tenant name assigned to it, one key per Tenant, because Tenant labels are not unique. `demo_reservations` is Tenant-owned: it reserves a fictional subject of the Tenant for a lowercase email address, unique per `(tenant, email)`, and a check keeps `spent_at` and `spent_by` (an External Identity) set together. It references the Tenant root record through `demo_reservations_tenant_root_fkey`. `identity/0034_demo_data_state` is the generated state-only checkpoint, and `identity/0035_demo_reservations_row_level_security` (after it and `identity/0028_tenancy_policy_functions`) enforces the forced Tenant policy on `demo_reservations` with a rendered `RunSQL` asset, as `registry/0040` does.

`execution/0049_foreign_key_relations_state`, `federation/0012_foreign_key_relations_state`, `identity/0039_foreign_key_relations_state`, `knowledge/0035_foreign_key_relations_state`, `registry/0044_foreign_key_relations_state` and `workspaces/0010_foreign_key_relations_state` (each after its app's previous leaf) are state-only. They record in the ORM snapshot the foreign keys that model fields now declare with `#[rel(foreign_key, shared_fields = ["tenant"], db_constraint = ...)]`, with the names, columns and actions that the `*_references` migrations and later migrations already created in PostgreSQL. A foreign key whose column is part of the primary key, or that is a generated column, stays a plain `_id` field and out of the snapshot. `registry/0044` is listed in `TENANT_REGISTRY_MIGRATIONS`.

`execution/0050_mcp_elicitation_kind` (after `0049_foreign_key_relations_state`)
adds `MCP_ELICITATION` to the physical human-request kind check with typed
Reinhardt constraint operations. `0051_mcp_elicitation_state` updates the ORM enum
state. Rollback's restored check refuses existing MCP elicitation rows rather
than deleting them. Deploy API servers and workers together before enabling
these requests; older binaries cannot decode the new kind.
`registry/0045_context_sources` (after `0044_foreign_key_relations_state`) validates the optional native `context_source` declaration and the required workspace-observation source (#110, ADR 0086). Exact Tool and Decider references remain admission and live-authority checks. The migration replaces PostgreSQL JSON validation functions using DDL because Reinhardt has no typed function-body operation. It changes no rows and rollback refuses while declarations requiring the new schema remain.

## Projection comparison evidence (#181)

`registry/0045_projection_measurement_config` (after
`registry/0044_foreign_key_relations_state`) adds the optional model
`provider_route` and `estimate_diagnostics` keys. It edits the current
`registry_model_config` allowlist from `pg_get_constraintdef`, retaining earlier
fields and refusing a missing anchor. An exact route records `tag`, `source`,
`verified_at` and `expires_at`; it is valid only for an OpenRouter model without
a Plan Connection. Diagnostics accept a supported Token Encoding and bounded,
per-projection calibration snapshots. These are observational inputs and do not
change an Agent's Projection Version or the runtime estimator. Separate
PostgreSQL validation functions and a named CHECK enforce their JSON shapes.
Catalog-driven constraint edits and PL/pgSQL function bodies have no typed
Reinhardt operation, so this database-only migration uses reversible DDL assets.
Definitions are immutable: rollback refuses with SQLSTATE `55000` while any model
definition contains either key, including an explicit null. Retain the migration
or restore the pre-upgrade database rather than rewriting registered versions.

`execution/0050_usage_evidence` (after
`execution/0049_foreign_key_relations_state`) extends `inference_usage` with
`purpose`, `requested_route`, `provider_identity` and `shadow_estimates` through
typed Reinhardt column operations. Historical records receive `purpose = 'main'`;
the migration then removes that default so a new dispatch supplies its purpose.
Main and Summary provider calls have distinct durable attempt records. Requested
routes are separate from bounded provider-reported identities; omitted provider
facts remain unknown. Shadow snapshots do not alter dispatch. The ordinary
column operations also update migration model state, so no separate state-only
checkpoint is needed. Rollback drops these evidence columns and loses their
contents; preserve required experiment exports first.

`execution/0051_compaction_provider_dispatch` (after
`execution/0050_usage_evidence`) adds nullable `provider_dispatched` to
`context_compaction_attempts` with a typed Reinhardt column operation. A Summary
journal entry is created before authorization and reservation, so its existence
alone does not prove provider I/O. New Summary attempts start false; the flag
becomes true in the same transaction as their durable Inference Usage dispatch.
Historical null remains unknown. Export consumers can exclude a proven
undispatched attempt from provider accounting, but must not treat null as zero
cost. The operation updates model state and requires no state-only checkpoint.
Rollback drops this evidence column and loses the distinction.

`knowledge/0036_memory_attempt_evidence` (after
`knowledge/0035_foreign_key_relations_state`) adds nullable `source_run_ids`,
`reserved_tokens`, `reserved_cost_micros` and `provider_usage` to
`memory_model_attempts` with typed Reinhardt column operations. These preserve
physical-call lineage, original reservations and actual provider observations
apart from settled policy tariffs. Historical rows remain null: the migration
does not invent old ownership or provider measurements. It also needs no
state-only checkpoint; rollback drops the added evidence columns and their
contents. Existing Tenant row policies continue to cover both attempt tables.

The [projection comparison guide](../../docs/projection-comparison.md)
describes the prompt-free diagnostic export, complete attempt inventories and
remaining live acceptance requirements. These migrations do not switch the
projection default or rewrite pinned definitions and Runs.

## Merged MCP and projection histories

`execution/0052_merge_mcp_and_usage_evidence` depends on both
`0051_mcp_elicitation_state` and `0051_compaction_provider_dispatch`.
`registry/0046_merge_agent_plugin_and_projection` depends on both
`0045_agent_plugin_settings` and `0045_projection_measurement_config`.
These empty merge migrations give each app one leaf while preserving the
original migration identities, dependencies, schema changes and rollback
guards. New migrations depend on the merged leaf. They perform no schema
operations and change neither retained MCP state nor projection behavior.

`registry/0048_plugin_marketplace_merge` joins the Registry merge above with
`0047_marketplace_recall_constraints`. This empty state-only migration keeps a
single Registry leaf without rewriting either history or changing stored data.
`execution/0058_research_projection_merge` and
`registry/0046_context_projection_merge` join the independent Research/approval
and projection evidence histories, and the Context Source and projection
configuration histories, respectively. Each empty state-only migration depends
on both existing leaves and changes no schema or data. Existing migration names,
dependencies and rollback guards remain intact; each app has one head for future
generation.

`registry/0048_context_marketplace_merge` joins that Registry history with
`0047_marketplace_recall_constraints`. This empty state-only migration keeps a
single Registry leaf without rewriting either history or changing stored data.

`execution/0059_research_history_merge` also joins
`0051_research_journal_results_state` to that Execution history. This empty
state-only migration preserves the durable Research result history and leaves
a single Execution head without changing either existing branch.

`execution/0106_node_research_merge` joins Node cancellation and Research/approval
histories with an empty state-only migration. Both existing histories and their
rollback guards retain their identities; no schema or stored data changes.

`execution/0060_plugin_research_merge` and `registry/0049_plugin_context_merge`
join the MCP and Research/approval histories, and the Agent Plugin and Context
Source histories, respectively. Both are empty state-only migrations. They
preserve the existing migration identities and give each app one leaf without
changing schema, retained inputs, or data.

`execution/0107_node_plugin_merge` joins the Node cancellation and MCP
continuation heads after their Research history merges. The empty state-only
migration preserves both histories and leaves one Execution head.
`execution/0084_invocation_tenant_secrets` (after `execution/0083_harness_contract_state`)
adds the nullable `tenant_secret_id`, `tenant_secret_version` and
`tenant_secret_host` of `invocations` with typed column operations (#310, ADR
0095). A tool call without a Tenant Secret leaves them null; a call that carried
one records which Tenant Secret, at which pinned version, went to which host,
never its value.

`execution/0085_egress_secret_records` (after `execution/0084_invocation_tenant_secrets`)
adds the nullable `secret_id`, `secret_version` and `secret_host` of
`egress_requests` with typed column operations (#310, ADR 0095). A request for
an External service without a Tenant Secret leaves them null; a brokered request
records the Tenant Secret, the pinned version its token names and the
destination host, never the value or a header value. The typed operations update
the ORM snapshot, so no state-only checkpoint is needed.

`execution/0119_egress_request_times` (after `execution/0118_blob_uploads`) adds
the nullable `admission_ms` and `settle_ms` of `egress_requests` with typed column
operations (#752). A record holds them once its outcome settles, so a record that
never settled and a record older than this migration leave them null. The typed
operations update the ORM snapshot, so no state-only checkpoint is needed.

`execution/0129_host_connection_limits` (after
`execution/0128_remove_generated_summary_state`) adds the nullable `error` and
`wait_ms` of `egress_scope_connections` and drops `suspended`,
`window_started_at` and `connections_in_window` from `egress_scope_state`, with
typed operations (#845). Nothing suspends a scope any more, so a suspended scope
opens again. A Host connection that held its limits records the code its client
received and its wait. Rows older than the migration leave the new columns
null. The reverse restores the state columns with their defaults, so every scope
comes back unsuspended. `execution/0130_scope_refusals` is database-only and
creates `egress_scope_refusals`, the per-minute counts of connections that ended
before they held a limit, with its index and the Tenant isolation policy that
`0110` gives the other scope tables; Reinhardt migrations cannot express row
policies. `execution/0131_scope_refusals_state` is its state-only checkpoint, as
`0111` is for `0109`.

## Home ledger recovery eligibility (#434)

`knowledge/0036_memory_recovery_eligibility` (after
`knowledge/0035_foreign_key_relations_state`) adds the required
`recovery_eligible_at` timestamp to `memory_bank_settings`. Existing rows receive
the migration timestamp, then the default is removed so application writers
explicitly record eligibility. Typed Reinhardt column operations preserve the
matching model state.

`knowledge/0037_recovery_evidence_merge` joins this history with
`knowledge/0036_memory_attempt_evidence` without changing either migration. It
is an empty state-only migration: both independent column changes apply before
the single Knowledge head, and no schema or model state is overwritten.

`execution/0097_provider_tool_usage` stores provider-reported tool counts. `registry/0061_provider_tools` extends the current descriptor and Agent binding validation with Provider tool contracts, preserving the prior functions by delegation.

`identity/0051_tenant_data_protection` adds each Tenant's current Data Protection Level and append-only setting history (#542). A missing row means Standard with model-call ZDR and the Plan Connection exemption enabled. Checks reject unknown levels and a Zero Data Retention policy with model-call ZDR disabled. `identity/0052_data_protection_row_level_security` applies the same forced Tenant-isolation policy as the other Tenant-owned tables; literal SQL is required because Reinhardt does not expose RLS policy operations.

`identity/0053_tenant_data_protection_state` records those two tables in ORM migration state. It executes no DDL; references, validity checks and row-level policies remain in the preceding database-only migrations.

`registry/0062_provider_warning_tools` extends the closed Provider tool validation functions with the warning-tier tools (advisor, subagent, model search, tool search, shell) without replacing the previous contracts; its reverse refuses to run while any such definition or pinned Run exists. `identity/0054_provider_warning_acknowledgment` adds the nullable exact disclosure text to `authorization_catalog_history`, filled only when a warning tool is approved. `execution/0098_provider_container` adds the nullable `runs.provider_container` pin (OpenRouter container id and network policy). OpenRouter has no container delete API, so the pin is kept after the Run ends.

`registry/0063_reference_set_limit` raises the most Reference files one Source can bind from 8 to 256 in the PostgreSQL context validator. The function body uses SQL assets because Reinhardt has no typed operation for replacing PostgreSQL functions. Reversal refuses while a Source or a Run snapshot holds more than 8.

`registry/0064_journal_tools` admits the built-in Journal descriptors while preserving the existing descriptor validation. PostgreSQL function DDL requires SQL assets. Reversal refuses while Journal definitions remain.
`registry/0067_optional_memory_roles` permits an Agent model role and an absent embedding in native memory definitions. Its PostgreSQL validation and reference trigger use SQL assets because Reinhardt cannot express function bodies. Reversal refuses while definitions use the new forms.
`registry/0076_delete_subnode_tool` admits the `delete_subnode` builtin, the renamed `subnode_dissolve` (#1012), judged exactly as the operation it replaces; `subnode_dissolve` stays declared for pinned Agents. `registry/0077_tool_parallelism_64` lets an Agent declare a Tool Parallelism of up to 64 (#1009), judging 17 to 64 as 16 with the previous validator. Both keep the previous body under a backup name, and their rollbacks refuse with SQLSTATE `55000` while stored definitions use what they admit.

`registry/0075_harness_contract_structured4` lets the Agent Bindings validator accept
`structured@4` (#1010), judging it exactly as `structured@3`; registration checks its Native pairing. It
copies the current body to `aidash_agent_before_structured4_0075`. Its rollback
refuses with SQLSTATE `55000` while definitions, packages or Run Binding
snapshots declare `structured@4`, then restores the `registry/0074_remove_research_providers` body.

`registry/0069_schedule_tools` admits the four `core.schedules@1` builtins (`schedule_create`, `schedule_list`, `schedule_update`, `schedule_delete`, #557) in `aidash_descriptor_is_valid`, and lets an Agent's `remove_default` name them in `aidash_agent_bindings_is_valid`, since they are default tools. It copies both current bodies to `aidash_descriptor_is_valid_before_schedules_0069` and `aidash_agent_before_schedules_0069`; a Schedule descriptor is checked with the established builtin rules of `core.tasks@1`, and every other `remove_default` entry by the previous rules, so nothing else is relaxed. PostgreSQL function bodies have no typed Reinhardt operation, so it is a reversible SQL asset. Reversal refuses with SQLSTATE `55000` while a registry tool declares `core.schedules@1` or an Agent removes a Schedule tool, then restores both previous bodies. It follows the Tenant Registry migrations of #243 and is listed in `TENANT_REGISTRY_MIGRATIONS`.

`registry/0073_agent_plugin_server_secrets` (after `registry/0072_remove_agent_external_service_draft_bindings`) lets an Agent definition name a pasted secret per remote server of an Agent Plugin in `agent_plugin_settings` (#954, ADR 0073 amended 2026-11-16). It wraps the Agent validator like 0061 and 0065: it checks and removes each server `secret`, then calls the earlier body. The function body is a SQL asset because Reinhardt cannot express PostgreSQL function bodies. Reversal refuses while definitions or Run snapshots name a server secret.

## Run control provenance and Safety Stops (#485, ADR 0101)

`execution/0075_run_control_provenance` (after `execution/0074_federated_preview_authority`)
adds `runs.control_revision` (bigint, default 0) and `runs.control_source` (text) with
typed column operations; existing Runs start at revision 0 with the Tenant as the
source. The model declares no default for the source, so the logical state drops it
the way `execution/0050_usage_evidence` does. The revision changes only when `control`
changes, so a Safety Stop's expected revision is not made stale by worker progress.

`execution/0076_run_control_trigger` is database-only. It restores the physical
`'tenant'` default of `control_source` for writers that name no source, and adds the
CHECK that the revision is not negative. Reinhardt has no trigger
operation, so the function `aidash_run_control_provenance` and its `BEFORE UPDATE OF
control` trigger on `runs` are the documented raw-SQL exception. A writer that updates
`control_revision` itself (the ordinary control path and a Safety Stop) is left alone;
every other writer that changes `control` gets the revision bumped and is attributed to
`system`. Its reverse drops the trigger and function.

`execution/0077_run_safety_stops` creates the audit record and replay receipt, one row
per accepted or no-op command, with typed operations. `run_id` has no foreign key,
because the record is audit history that outlives the Run, its Workspace and the purge
of its Tenant (it is classified `audit`, erased by `audit_retention_days`). Its Tenant
reference is the usual root record. `(tenant, caller_key, operation_id)` is unique, and
CHECKs bound the reason to 1 to 1000 characters and tie `identity_id` to a browser actor.
`execution/0078_run_safety_stops_row_level_security` adds the tenant policy template
through `RunSQL`, and `execution/0079_run_safety_stops_state` is the generated state-only
snapshot that leaves the physical constraints and indexes to `0077`.

`workspaces/0015_attachment_objects` adds an immutable object reference and
optional decoded-media measurements to Thread Attachments. Existing inline bytes
remain readable until startup backfill verifies and moves them; new attachments
use `core_objects`. PostgreSQL nullability is changed with `RunSQL` because the
pinned typed `AlterColumn` cannot express it (#6803). Reversal refuses while any
attachment has no inline content. `0016_attachment_content_state` records the new fields and nullable bytea in
ORM state. The object reference is Tenant-scoped and cascades
on object deletion, allowing a deleted Tenant's object purge to run before row
purge. The object store's intents recover uncommitted files after a crash.

`workspaces/0017_attachment_wav_rendition` adds a nullable, tenant-scoped immutable WAV rendition reference and its integrity metadata. The three fields are set together when a route first needs the rendition; deleting the attachment makes both objects reclaimable. `0018_attachment_wav_rendition_state` records the fields in model state.
`execution/0086_invocation_connections` (after `execution/0085_egress_secret_records`)
adds nullable `invocations.connection_id` with a typed Reinhardt operation (#554,
ADR 0100). It records the person's OAuth Connection used by dispatch, never a token.

## Harness Contract versions (#531)

`registry/0050_harness_contract` (after `registry/0048_context_marketplace_merge`
and `registry/0045_agent_plugin_settings`)
replaces `public.aidash_agent_bindings_is_valid` so the optional
`harness_contract` key accepts `legacy@1`, `structured@1` or `structured@2`;
omitted and explicit null are accepted so definitions written before #531 keep
their bytes. `AgentBindings::validate` rejects a structured version whose
Projection Version is not Native. PostgreSQL function bodies have no typed
Reinhardt operation, so the replacement and its 0045 restore live in reversible
SQL assets. Rollback refuses while definitions, packages, or Run Binding snapshots declare the key; restore the pre-upgrade backup to recover. It is database-only and changes no model state.

`execution/0082_harness_contract` (after `execution/0081_lease_recoveries`)
adds `inference_usage.harness_contract` with typed column operations. Historical
Usage Records receive `1` (`legacy@1`); the migration then removes the default so
every new dispatch supplies the pinned version, and a check keeps it positive.
The generated state-only checkpoint `execution/0083_harness_contract_state`
drops that check from the ORM snapshot again, because it is enforced apart from
model state like the earlier `inference_usage` checks. Rollback drops the
evidence column and loses the recorded versions.

`registry/0055_remove_structured_harness_contracts` (after
`registry/0054_remove_agent_cluster`) stops the Agent Bindings validator from
accepting the removed `structured@1` and `structured@2` (#532). Stored pins are
not rewritten, because a Run Binding snapshot keeps a serde digest of each pinned
definition that SQL cannot recompute; before any DDL, the migration refuses
while Agent definitions, Marketplace packages or Run Binding snapshots still
declare either version. Rollback restores the 0054 body.

`registry/0052_harness_contract_structured3` lets the Agent Bindings validator accept
`structured@3` (#532). Its rollback restores the 0050 body and refuses while
definitions, packages or Run Binding snapshots declare `structured@3`.

`registry/0051_harness_plugin_merge` joins the Harness Contract and Agent
Plugin/Context Source heads. The Harness Contract validator extends the Agent
Plugin settings validator, including its compaction rules, and restores that
validator on rollback. The join is an empty state-only migration that preserves
existing identities and one leaf per app.

## Remote reads record whether the Node is the reader's own Tenant's

`identity/0070_remote_reads_same_tenant` (after `identity/0069_decision_coalescing_indexes`) adds `same_tenant boolean NOT NULL DEFAULT false` to
`authorization_run_remote_reads`; `identity/0071_remote_reads_same_tenant_state` is its ORM state.
A row written before it is treated as another Tenant's, which keeps asking that Node (the stricter
check), so nothing needs backfilling. Rollback drops the column. A read of an Agent on a Node of
the reader's own Tenant is then decided by `registry.read` and `federation.discover` alone (#769).

## Merged Node lifetime and Auth histories

`identity/0066_node_auth_history_merge` joins the Common Auth origin binding,
remote terminal receipts and task source lifetime histories.
`execution/0105_node_lifetime_merge` joins Node cancellation jobs with Session
Area lifetime state. `knowledge/0050_recovery_lifetime_merge` joins Home ledger
recovery eligibility and attempt evidence with remote-read source lifetimes.
Each is an empty state-only migration that joins its listed histories. The
renumbered Node chains also follow the provider and semantic image histories,
leaving one leaf each for Identity, Execution and Knowledge.

`knowledge/0051_recorded_memory` permits automatically recorded facts and
observations without marking them as awaiting human verification.
`knowledge/0052_memory_sharing_scope` stores the Tenant Administrator's
revisioned Workspace or Tenant-Agent choice and enforces Tenant row isolation.
`knowledge/0053_memory_journal_proofs` retains Research identities from
admitted Journal ranges independently of their content, with Tenant isolation.

`execution/0108_node_preview_merge` joins `execution/0107_node_plugin_merge`
with the Live preview, Safety Stop, Lease Recovery and Harness Contract history
ending in `execution/0083_harness_contract_state`.
`federation/0021_node_preview_origin_merge` joins Node registration recovery
ownership with `federation/0013_registered_preview_origin`. Both are empty
state-only migrations; existing schema changes and rollback guards remain intact.
Federation retains its pre-existing separate `0007_model_state` leaf alongside
`0021_node_preview_origin_merge`.

`identity/0067_scratch_member_policy` uses a Reinhardt Query-generated data asset to add `scratch.create` only to unchanged default member policies. It opens Deployment Scope before updating Tenant policy bundles and increments their revisions. Its reverse is a no-op: later policy edits cannot be distinguished safely from the migrated grant, so rollback preserves them. `migrationseeds` checks both Query-generated assets for drift.

`identity/0072_remote_execution_activation` adds the nullable `authorization_remote_execution.activated_at` (#772). The Home sets it after the receiving Node's activation response matched, so a binding left behind by a refused activation is not read as a Run that started. One Reinhardt `AddColumn` carries both the table and the model state; nothing is backfilled, because nothing is deployed. A binding counts as live only while its `task_revision` equals the Task's revision, so a Task reopened after its remote Run ended is an Unassigned Task again.

`identity/0077_provider_credential_origin` adds `provider_credentials.key_origin` (#892): `authorization`, `pasted` or `host_access`, saying how the pinned Key Material arrived. A Reinhardt `AddColumn` with the default `'authorization'` fills existing rows, which all came from Provider Authorization, and an `AlterColumn` then drops the default so every write names its origin.

`identity/0080_schedule_member_policy` adds `schedule.create` (#557) to unchanged default member policies with a Reinhardt Query-generated data asset, as `identity/0067_scratch_member_policy` added `scratch.create`. It opens Deployment Scope, increments the revisions of the bundles it changes, and leaves custom member policies alone. Its reverse is a no-op for the same reason as 0067's. Both builders pin the member policy as it stood then (`duty_policy_backfill::member_policy_after`: no Thread actions, no `subnode.create`, and `schedule.create` rather than `duty.create`), so later grants and renames do not change their assets; `migrationseeds` checks both.

`execution/0145_stints` and `0146_stints_row_level_security` add `stints` (#1009, ADR 0141): one row per Agent's awake stretch on a Task, with the Agent version, Node, start, end and why it ended. A trigger on `runs` opens a Stint when the Run becomes ready or starts working and none is open, and ends it when the Run enters any wait but failure delivery (the reason is the wait's name) or reaches a terminal phase; a partial unique index keeps one open Stint per Run. `inference_attempts.stint_id` is filled by a trigger from the open Stint. Reinhardt has no trigger operation, so the functions and triggers are the documented raw-SQL exception. 0146 is the rendered Tenant-owned row-level policy asset. Stints go with their Run on Node purge.

`execution/0152_stint_leases` adds `stints.lease_owner` (the last holder's claim token, kept after it releases the lease) and `stints.lease_recoveries` (integer, 0 by default) with typed `AddColumn`s, and replaces the body of `aidash_stint_transition` (#1009, ADR 0141 amendment of 2026-11-20). The trigger now also fires on `control` and `lease_owner`: a claim whose token replaces one that was never released counts a Lease Recovery in the open Stint; a pause (`control = 'PAUSED'`, by a person, a Budget set to "stop" or a Safety Stop) ends the Stint with reason `paused` and opens none while paused; only an `ACTIVE` execution opens one, so a resume opens the next Stint and a paused execution that is then cancelled opens none. Replacing a PL/pgSQL function and re-creating its trigger have no typed Reinhardt operation, so the function and trigger are the documented raw-SQL exception; the reverse restores the 0145 body and trigger before the typed reverse drops the columns. Nothing is backfilled, because nothing is deployed: open Stints of executions paused before this migration stay open until their next transition.

`execution/0143_subnode_caps_removed` and `0144_subnode_caps_removed_state` drop `subnode_limits.members_per_subnode`, `subnode_limits.subnodes_per_run` and their range check (#1012, ADR 0143): Subnodes nest and are no longer capped per Run or in members. Only `remote_runs_may_create` remains, which nothing reads since the Federation tool lists took over the receiving Tenant's consent (ADR 0139). The reverse restores both columns with their former defaults (5 and 3) and the range check.

`identity/0091_standing_instructions`, `0092_standing_instructions_row_level_security` and `0093_standing_instructions_state` add `tenant_instructions` (one per Tenant) and `node_instructions` (one per Node, foreign key to `nodes`) for #1010: the text, at most 16,384 bytes (PostgreSQL text cannot hold NUL; the application refuses it first), a positive revision for optimistic updates, and who changed it when. 0092 is the rendered Tenant-owned row-level policy asset; 0093 records the ORM state. Node Instructions go with their Node on purge.

`identity/0094_tenant_execution_settings`, `0095_tenant_execution_settings_row_level_security` and `0096_tenant_execution_settings_state` add `tenant_execution_settings` for #1009: the Tenant's Lease Recovery limit (1 to 20; a Tenant with no row uses 5), a positive revision for optimistic updates, and who changed it when. 0095 is the rendered Tenant-owned row-level policy asset; 0096 records the ORM state.

`identity/0081_scheduled_executions` adds `authorization_execution.scheduled` (#557, ADR 0134), false by default. The scheduler sets it on the Run a tick starts, and every admission by a Run that carries it marks the admitted Run too, whatever Task it delegates, so no Run descended from a Schedule registers or changes Schedules. One Reinhardt `AddColumn` carries both the table and the model state; nothing is backfilled, because nothing is deployed.

`execution/0132_generation_scheduled` adds `generation_requests.scheduled` (#557, ADR 0134), false by default. A generation request made by a Run that carries the mark above carries it too, and the Run its generated Agent is delegated takes it on, so `task_assign` cannot lead out of a Schedule either. One Reinhardt `AddColumn` carries both the table and the model state.

`workspaces/0029_schedules` creates `schedules`, `schedule_ticks` and `schedule_limits` with typed operations (#557, ADR 0134). A Schedule belongs to one Workspace through `(tenant, workspace_id)` and is deleted with it; its ticks go with it. `schedules_due` is the partial index the scheduler of every replica reads, and `schedule_ticks_slot_key` is the unique key `(schedule_id, scheduled_at)` of scheduled ticks that keeps a scheduled time to one Task across restarts and replicas ("Run now" ticks, which carry `started_by`, are outside it). Checks keep a halt's reason, its time and a successor consistent. `halt_reason` keeps a stop's reason in words, and `creation_key` with `created_by_run_id` (unique in `schedules_creation_key`) lets an Agent's retried tool call find the Schedule its first call registered. `workspaces/0030_schedules_row_level_security` enforces the forced Tenant policy on the three tables with the rendered policy template, and `workspaces/0031_schedules_model_state` is the generated state-only checkpoint.

`workspaces/0032_duties` turns Schedules into Duties (#1011, ADR 0142) with typed operations: `schedules` and `schedule_ticks` become `duties` and `duty_ticks`, `registrant` becomes `requester`, `request` becomes `mission`, `schedule_id` becomes `duty_id`, a Duty gains `procedure` and `pause_after_failures` (3 for every existing Duty; 0 never pauses it), and `trigger` may be null for a Duty without a time trigger. Index and key names keep their former prefixes. `workspaces/0033_duty_checks` replaces the halt, successor and trigger checks (`requester_left` replaces `registrant_left`, and the minimum-interval halt is gone with the minimum itself, #1012), bounds `pause_after_failures` to 0–1000, rewrites the stored halts with the Reinhardt Query output of `workspaces::services::duty_halt_backfill` (a Duty stopped by the minimum interval is paused), and drops `schedule_limits`, whose forced row-level policy a typed `DropTable` cannot drop or restore. `workspaces/0034_duties_state` records the dropped table in the ORM state.

`workspaces/0035_duty_triggers`, `0036_duty_triggers_row_level_security` and `0037_duty_triggers_state` add the Duty Triggers other than the clock (#1011, ADR 0142): `duty_triggers` (a webhook, or another Duty's Task ending, named by `source_duty_id`; a check keeps the two kinds apart and a Duty from waking on itself), `duty_events` (each event's payload as JSONB of at most 1 MiB, what it did and the Task or Run it reached; they go with their Duty) and `duty_bookings` (a Run's booking of the next event of a trigger, unique per trigger and Run, deleted with its trigger). Keys, checks and indexes are database-only in 0035; 0036 is the rendered Tenant-owned row-level policy asset; 0037 is the generated ORM state. No signing secret is stored: a webhook's secret is derived from the Deployment's API token.

`identity/0097_unscheduled_executions` and `execution/0147_unscheduled_generations` drop the `scheduled` marks of 0081 and 0132: a Run a Duty started may set up Duties like any other (#1012). `identity/0098_duty_member_policy` renames `schedule.create` and `schedule.administer` to `duty.create` and `duty.administer` in every stored policy and gives the unchanged default member policy `subnode.create` (#1012); its assets are the Reinhardt Query output of `identity::services::duty_actions_backfill`. The rename leaves `duty.create` where `schedule.create` stood in the sorted action list, and JSONB array equality is ordered, so the default policy is recognized by containment both ways (as a set), not by `=`. `registry/0078_duty_tools` admits the `core.duties@1` builtins `duty_create`, `duty_list`, `duty_update` and `duty_delete`, judged exactly as the Schedule tools they replace, and lets an Agent remove them from its defaults; its rollback refuses with SQLSTATE `55000` while stored definitions use them.

`identity/0099_task_origin_federated` adds `authorization_task_origins.federated`. `insert_origin` records whether the creating chain passed through another Node or can no longer be proven Local, so admission reads one row instead of walking a chain that no longer has a length cap (#1012). Aidash is undeployed, so existing origins read as Local until recreated.

`workspaces/0043_duty_trigger_from_node` adds the nullable `duty_triggers.from_node` (in the ORM model) and `workspaces/0044_duty_trigger_node_event_check` (database-only) widens `duty_triggers_kind_check` to the `node_event` kind, an event another Node sends (#1013, ADR 0142): only that kind may name `from_node`, the one Node whose events it takes.

`identity/0100_budgets` creates `budgets` (one per Tenant, Node, Duty or person scope), `budget_notices` (the Soft Limit mail sent per window) and `budget_spend` (the spend ledger: one row per UTC day, Node, root person and Duty, the nil UUID for no Duty) (#1012, ADR 0143). The ledger has no key to Runs or Duties, so deleting a Duty or a Thread, or purging a Node, gives no money back to the Budgets that covered it. `identity/0101_budgets_row_level_security` enforces the Tenant row policy on all three. The tables are written and read with Reinhardt Query and have no ORM model. `workspaces/0038_task_duty` adds the database-only `tasks.duty_id`, set once when a Duty starts its Task and copied to every Task a Run of that Task creates, so a Duty's spend is charged without walking Task trees.

`registry/0079_workbench_input_bytes` raises the default Workbench test limits from 20,000 to 40,000 input bytes (`max_input_bytes`) and from 16,384 to 32,768 total tokens (`max_total_tokens`): the default tools a test declares, the Duty tools of #1011 and the longer `workspace_wait` included, no longer fit in 20,000. Stored Tenant limits keep their values.

`registry/0080_node_tools` admits the `core.nodes@1` builtins `node_create` and `node_delete` (#1012) and the `core.duties@1` builtin `duty_notify` (#1013), which an Agent binds explicitly. Their descriptors have the shape of the Duty tools and are judged by those rules. Its SQL replaces a PostgreSQL validation function, which Reinhardt migrations cannot express; the rollback refuses with SQLSTATE `55000` while stored definitions declare them.

`workspaces/0039_duty_scripts`, `0040_duty_scripts_row_level_security` and `0041_duty_scripts_state` add a Duty's Router and Watcher scripts (#1011, ADR 0142) with typed operations: `duties.router` and `duties.watcher` (JSONB, checked for a known language and a string source), `duties.watcher_next_at` with a partial index for due Watchers, `duty_events.router` (the Router's decision) and the outcomes `routing`, `dropped` and `acted`. `duty_script_versions` keeps every saved script with its author and time, and `duty_script_runs` is keyed on the Task a Duty started to run a script without a model: its frozen source, its stdin and when the Duty applied its outcome. Both are keyed on the Duty and cascade with it, with no `workspace_id`; their sources are at most 64 KiB. Keys, checks and indexes are database-only in 0039; 0040 is the rendered Tenant-owned row-level policy asset; 0041 is the generated ORM state.

`identity/0109_agent_created_nodes` (after `identity/0101_budgets_row_level_security`) lets an Agent's chain create and delete Nodes as its root person (#1012, ADR 0143). It adds the database-only `nodes.created_by_subject`, `created_by_run_id`, `created_by_task_id` and `creation_key`, which only an Agent-created Node (`created_by_kind` `agent`) sets: the `nodes_creator` check now admits `agent` and requires all four for it. The unique `nodes_creation_key` index lets a retried tool call find the Node its first call created, and `nodes_created_by_task` serves a flow's stop. The `node_deletions_state` check admits `deleted_by_kind` `agent`. The ORM model never sets the new columns; their writers and readers use Reinhardt Query. `workspaces/0047_duty_creating_task` (after `workspaces/0038_task_duty`) adds the database-only `duties.created_by_task_id`, the Task whose Run set the Duty up, with the `duties_created_by_task` index, so stopping a flow pauses the Duties its Runs set up; a successor taking a Duty over clears it. Existing rows stay empty and are found through `created_by_run_id`. Both use typed operations and reverse cleanly while no Agent-created Node or deletion exists.

`identity/0112_budget_lineage` (after `identity/0111_thread_member_policy`) creates the Tenant-owned `budget_lineage` table for nested Budgets (#1012 decision 33, ADR 0143): a Duty a Duty's Run creates, and a Node a Run on a Node creates, records every Duty or Node above it once, when it is created (`kind` `duty` or `node`, `child`, `ancestor`). A Duty or Node Budget then covers the work below it and counts its spend. Like the spend ledger, the lineage keeps no key to Duties or Nodes, so deleting a Duty or purging a Node gives no money back to the Budgets above it; Node purge keeps it. It is written and read with Reinhardt Query and has no ORM model. Its Tenant row policy is a SQL asset in the same migration, because Reinhardt migrations cannot express policies. It reverses cleanly; the lineage is lost.

`workspaces/0049_threads` dissolves the Workspace into Threads (#1008, ADR 0140). It depends on the leaf of every app whose history names the `workspaces` table (all but `marketplace` and `operations`). The table becomes `threads` (same ids) without `goal` and `state`, every `workspace_id` of the app becomes `thread_id`, `duties` gains `origin_thread_id` (the Thread a Duty was registered from; empty for existing Duties, which keep the Thread they were registered in as their own) and `threads` gains `deleted_at`. Reply threads go: Aidash is undeployed, so the migration **drops** them rather than converting them or refusing to run. `channel_threads` rows are deleted with the table, replies stay as flat messages of their Thread, and the columns that named a reply thread are dropped (`channel_message_context.thread_id` with its `channel_reply_lookup` index, `duties.thread_id`, `duty_ticks.thread_id`). The table drop, constraint and index renames and the dependency trigger body are SQL, which Reinhardt cannot express. `workspaces/0050_threads_state` removes `channel_threads` from the ORM state. `execution/0148_threads` renames the app's `workspace_id` columns, re-creates the function bodies that named them, and keys a Working Area on (Tenant, home Node, Thread, Agent, owner): `core_areas.thread_id` (a reply thread, conversation or Root Task) goes, Areas that differed only by it are merged into one per key (Reinhardt Query output of `workspaces::services::thread_backfill`): the survivor is the first active or running Area, else the first retained or recoverable one, else any, the newest first by its last admitted Run; the Runs, operations, objects, records, previews, message file versions and image publications that named a merged Area name the survivor, its merged Runs queued after the survivor's own; the merged Areas' manifests, retention holds and Live previews cascade, so files kept in a merged Area are dropped, and the Task to reply-thread map `core_task_sessions` is dropped; `execution/0149_threads_state` removes it from the ORM state. `identity/0110_threads` renames `authorization_workspaces` to `authorization_threads` and the app's `workspace_id` columns, and deletes every Thread Approval Mode (they named reply threads). `knowledge/0058_threads` and `federation/0030_threads` rename their `workspace_id` columns and function bodies (`federation/0030_threads` follows `federation/0029_run_only_tool_answers_state`). `identity/0111_thread_member_policy` renames `workspace.*` to `thread.*`, the `workspace` resource kind to `thread` and the `workspace_id` attribute (and its `/workspace_id` pointer) to `thread_id` in every stored policy; its assets are the Reinhardt Query output of `identity::services::thread_policy_backfill`. All of them reverse in schema only: reply threads and the Task map come back empty, each restored Workspace takes its title as its goal, each restored Working Area gets a fresh session id, and merged Areas and deleted Approval Modes stay gone.

`identity/0113_budget_notice_delivery` makes a Budget's Soft Limit notice recoverable (#1012). Pending notices store the spend that crossed the limit and an expiring delivery claim. Existing notices retain their timestamps. A successful SMTP delivery sets `sent_at`, and a window without SMTP is recorded as skipped. Reversal preserves sent notices and drops pending and skipped ones using a Reinhardt Query asset checked by `manage migrationseeds --check`.

`registry/0081_thread_tools` admits the `core.threads@1` builtins `thread_read`, `thread_observe`, `thread_search`, `thread_message` and `task_wait`, the default tools that replace the `core.workspace@1` ones (#1008), with exactly the rules of the tools they replace, and lets an Agent remove them from its defaults. The Workspace tools stay valid: saved Agent versions pin them. Its SQL replaces PostgreSQL validation functions, which Reinhardt migrations cannot express; the rollback refuses with SQLSTATE `55000` while stored definitions declare the Thread tools.

`registry/0082_thread_source_scope` lets the native memory Source validator (`aidash_native_context_before_sources`, behind `aidash_native_context_is_valid`) accept the scope `thread`, which Sources name since #1008, beside `participant` and `workspace` (stored versions, which Rust reads as the Thread). Nothing enforces that validator on stored rows: `registry_context_config` skips native configurations (no `schema_version`) since `registry/0015_binding_memory_merge`, and its only other caller, the `packages` check, went with that table in `registry/0026_remove_legacy_installations`. The migration only keeps the function consistent with what the code writes. The function body has no typed Reinhardt operation, so it is a reversible SQL asset copied from `registry/0067_optional_memory_roles` with the extra value; the reverse restores the 0067 body.

`execution/0150_task_executions` retires the Run's table name (#1009, ADR 0141): `runs` becomes `task_executions`, the Task's execution record, with the same rows, policies and triggers. It depends on the migration of every app whose history names `runs` (`identity/0111_thread_member_policy`, `knowledge/0058_threads`, `federation/0030_threads` and `registry/0081_thread_tools`; the knowledge leaf `0059_threads_state` is state-only and names no table), so reversing one of them reverses this one first: their reverse guards and function bodies read `runs` by name. The `runs_*` keys, checks and indexes, and `activation_dependency_runs` and `dashboard_status_waiting_runs`, take `task_executions` names; renaming them, and re-creating the seven function bodies that name the table (`aidash_activation_trigger`, `aidash_request_activation`, `aidash_thread_task_activation`, `gate_legacy_run_message`, `gate_legacy_run_output`, `legacy_run_input_bridge` and federation's `gate_remote_task_terminal`), are SQL, which Reinhardt cannot express. The ORM names an enum-domain check after its table, so the physical `control_source` check is dropped and added as `task_executions_control_source_model_enum_check` with typed operations, and `execution/0151_task_executions_state` does the same in ORM state for the `phase` and `control` checks, which exist physically as `task_executions_phase_check` and `task_executions_control_check`. Stored event kinds `run.*` become `execution.*` in Deployment Scope; the update is Reinhardt Query output from `execution::services::execution_event_backfill`, checked by `migrationseeds`, and its reverse turns every `execution.*` kind back into `run.*`. Kept on purpose: the `run_id` columns of other tables, their foreign-key names, the `run_*` table names, the trigger names (`gate_legacy_run_worker`, `aidash_run_control_provenance`, `aidash_stint_transition`), the policy actions `run.read`, `run.message`, `run.control`, `run.decline_call` and `run.finish`, and the resource kind `run` are internal names, like the Rust types (`Run`, `RunRecord`, ...). Only `events` rows are rewritten: digested records such as Context Journal entries keep whatever they quoted.

`execution/0153_area_queue_sleep` (after `execution/0152_stint_leases`) lets an execution hold its Working Area's queue only while it is awake (#1008, ADR 0140 amendment of 2026-11-20). `core_runs.asleep` (typed `AddColumn`, default `false`) marks an execution that sleeps in a wait that ends its Stint (every wait but failure delivery, the rule `aidash_stint_transition` follows) or is paused, once no worker holds it: the executions queued behind it run meanwhile, and the lease puts it back at the tail of the queue when it wakes. Existing executions are marked in Deployment Scope by Reinhardt Query output from `execution::services::area_queue_sleep_backfill`, checked by `migrationseeds`. The backfill and trigger both use `aidash_area_queue_should_sleep` to decide which execution states yield the queue. The eligibility function, the trigger `aidash_area_queue_sleep` on `task_executions` that sets the mark, the function `aidash_area_queue_release` that notifies the executions no awake one precedes any longer, and the re-created `aidash_activation_trigger` that calls it when an execution ends are the documented raw-SQL exception: Reinhardt has no operation for a function body or a trigger. Reversal restores 0150's activation trigger and drops the mark, so every execution queues behind the first of its Area that has not ended, as before.

`execution/0154_worker_stints` opens Stints at authorized worker entry. A lease acquired while a Run is waiting for its Working Area creates no Stint. The SQL replaces the trigger function that tracks lease holders and ends Stints, including a Stint already open when its Run has to queue. Reinhardt has no typed operation for a PostgreSQL function body. Reversal restores the phase-based trigger from `execution/0152_stint_leases`.

`execution/0155_model_calls` (after `execution/0154_worker_stints`) adds
prompt-free Model Calls and their links to Compaction Attempts, Inference
Attempts and Usage Records (#1068, ADR 0144). A recorded call names a Tenant
and Node and may name a Stint of the same Tenant and Run. The insert trigger
associates a step with its open Stint and requires exactly one owner on each
new Inference Attempt and Usage Record. Rows from older versions may lack
attribution and remain readable. Reinhardt has no typed operation for the
ownership trigger or Tenant policy, and its PostgreSQL migration renderer
omits changes in column nullability. The SQL assets therefore implement
those three changes. `execution/0156_model_call_state` records the optional
Run relation and Model Call ID in the Usage Record model state. Rollback
requires background Usage Records to be cleared before restoring the
required Run relation.

`execution/0157_generation_subnode_cleanup` adds the database-only
`generation_requests.subnode_cleanup_at` and a partial index for pending work.
Retirement sets the timestamp in its transaction. The generation worker clears
it after checking the retired version's Subnodes or reschedules it if cleanup
fails.

`workspaces/0019_scratch_spaces` adds Node-owned Scratch Spaces, their Tenant-scoped settings, and membership. Composite foreign keys keep each child and its parent in the same Tenant. `0020_scratch_spaces_row_level_security` applies the standard Tenant row policies. Reinhardt has no row-policy migration operation, so this migration uses the reversible policy SQL template. `0021_conversation_scratch_space` fixes each conversation to one Scratch Space in the same Tenant. `0022_scratch_space_model_state` records the ORM relationship state without changing the database-enforced constraints. The Query-generated `migrationseeds` assets backfill conversations before enforcing the binding. `0024_workspace_scratch_space` moves Run resolution to Workspace bindings, creating a dedicated space for each Workspace. It recovers the starter from Workspace ownership, a conversation, or the earliest Task. `0025_scratch_workspace_model_state` records the ORM relationship without changing the physical foreign key. A cleared space retains a tombstone while conversations still reference it; runtime reclamation is delivered separately.

`execution/0094_subnodes`, `0095_subnodes_row_level_security`, and
`0096_subnodes_state` create the Run-scoped Subnode tables, enforce Tenant
row policies, and record their model state after `0089_remove_agent_cluster`.

`registry/0060_subnode_provider` (after `registry/0059_reference_image_byte_state`) extends the PostgreSQL built-in Tool descriptor
validator with the six `core.subnodes@1` operations (#660). Its reversible
`RunSQL` replaces the PostgreSQL function body because Reinhardt migrations
cannot express PL/pgSQL function definitions. Rolling back restores the
previous descriptor contract; existing Subnode descriptors then cannot be
rewritten until the migration is reapplied.

`registry/0065_agent_default_removals` admits `connection_request` as a removable default and rejects the removed `compaction` default. Its SQL replaces a PostgreSQL validation function that Reinhardt cannot express; reversal refuses definitions or Runs that rely on the new removal.

`execution/0123_remove_compaction_decision_storage` removes the compaction-only decision state and outcome tables; `0124_remove_compaction_decision_storage_state` updates the model state. Decision Attempts and approval Evidence remain.

`execution/0125_remove_generated_compaction` removes generated-chain Decider usage and its obsolete counters; `0126_remove_generated_compaction_state` updates the model state. Its SQL drops the usage table's statement trigger and row policy, which Reinhardt migrations cannot express, and drops the table itself because a typed `DropTable` would reverse the historical composite primary key as two separate keys; the reverse SQL recreates the original key.

`execution/0127_remove_generated_summary` removes the generated Summary Stage allowance, its counters and the `generation_summary_usage` journal; `0128_remove_generated_summary_state` updates the model state. Its raw SQL has the same two reasons as 0125: the statement trigger and row policy have no typed operation, and the table drop must reverse to the original composite primary key. The Embedding allowance remains.

`knowledge/0054_memory_constraint_state` keeps database-only memory checks and Tenant foreign keys out of model migration state while retaining the constraints in PostgreSQL.

`registry/0066_native_context_recovery` replaces the Agent bindings validation function so a `context_policy` may carry the native recovery fields and a `summary` object that names no model of its own (it is validated with the Agent's model). Its SQL keeps the previous function body and delegates to it; PostgreSQL function bodies are DDL that Reinhardt migrations cannot express. Reversal refuses while definitions use the new policy fields.

`registry/0068_reject_new_compaction_deciders` adds a `NOT VALID` check that preserves existing compaction Decider rows while rejecting new or updated ones. PostgreSQL `NOT VALID` constraint DDL is expressed as raw SQL because Reinhardt does not provide it.

`identity/0073_invitation_direct_join` stores the issuer Mapping or bearer fingerprint and direct-join choice on Invitations and records approved groups in administration history. Existing Invitations retain the pending-request behavior. `identity/0074_invitation_notification_index` indexes the Tenant and action with the event time for the bounded notification feed.

`identity/0078_bootstrap_reservations` records each Tenant reservation source and adds a Deployment-wide Host Access email reservation for the first sign-in. It allows one Demo and one Host Access reservation per Tenant and email, and extends the Tenant and Node creator checks to accept `host_access`; `identity/0079_bootstrap_reservations_state` records the new field and table in ORM state, keeping physical checks out of that state. Rollback removes pending Deployment Administrator reservations and the reservation source column, and refuses while Tenant or Node records name Host Access as their creator or when Demo and Host Access reservations share an address in a Tenant.

`workspaces/0028_attachment_description` adds the nullable `channel_attachments.description` (#622 part 2): the Image Description that the person or Agent who posted an image wrote for it. One Reinhardt `AddColumn` carries both the table and the model state. Nothing is backfilled; an image posted earlier has no description until its poster writes one. Thread Attachment images are indexed in the existing `semantic_entries` under the key `attachment-image:<attachment id>`, so no knowledge table changes.

`registry/0072_remove_agent_external_service_draft_bindings` (after `registry/0071_execution_pod_tools`) removes `external_service` Bindings from Agent drafts (#738): External services are made available to a Scratch Space only, and an Agent no longer binds one. Rewriting a JSON array inside `agent_drafts.entry` is a one-time data change that stays SQL instead of a Reinhardt Query-generated asset: Reinhardt Query's FROM sources are tables and subqueries only, so it cannot express `jsonb_array_elements(...) WITH ORDINALITY AS item(binding, ordinal)`, and its function calls take plain arguments, so `jsonb_agg` cannot carry the `ORDER BY` that keeps the remaining Bindings in order. Reversal leaves the drafts as they are. Registered Agent versions are immutable and digest-pinned: they keep the Binding, which Run start, Approval Review and Marketplace publication leave out. The Binding contract function is not changed, so an imported older definition still inserts; new registrations are refused by the application.

`knowledge/0055_attachment_images` (after `0054_memory_constraint_state`) widens the `semantic_entries_counters` CHECK so a semantic entry may name a Thread Attachment image: `{"kind":"attachment_image","message_id":…,"attachment_id":…}` with exactly those keys and UUID strings (#622 part 2). Reinhardt can only replace a named CHECK by dropping and re-adding it, so the migration repeats the `0041_working_images` expression and adds one alternative. It is database-only, like `0041`. Rolling back fails while any attachment image entry remains.

`registry/0070_remove_package_index_draft_bindings` (after `registry/0069_schedule_tools`) removes every Binding to the removed package indexes `core.npm` and `core.pypi` (#745): in Agent drafts, in registered Agent versions (`config.bindings` and the normalized copy in `binding_normalization.bindings`, so the saved normalization still matches) and in registration requests. Like `registry/0054`, it rewrites Marketplace copies with the Marketplace writer fence disabled inside its transaction, and the reference tables follow through their triggers. Removing elements from a JSON array inside a column needs `jsonb_array_elements ... WITH ORDINALITY` and `jsonb_agg`, which Reinhardt Query cannot express, so this one-time data change is SQL. A rewritten Agent version has a new definition digest, so a Run snapshot, Marketplace version or Federation pin taken before the migration no longer matches it; nothing is deployed, and Agents that bound the indexes start again instead of being refused for the deleted Catalog Approval. The `core.npm` and `core.pypi` definition rows stay, unbound and unapproved. Reversal does not restore the Bindings.

`registry/0071_execution_pod_tools` admits the built-in descriptors of the Execution Pod groups (#745): `file_save`, `file_remove` and `file_copy` of `core.scratch@1`, `process_list`, `process_log` and `process_stop` of `core.processes@1`, and `mcp_stdio` of `core.mcp_stdio@1`, all Sandboxed and without a lifecycle. Without it every Tenant creation fails the `registry_tool_config` check. Its SQL keeps the previous function body and delegates to it; PostgreSQL function bodies are DDL that Reinhardt migrations cannot express. Reversal refuses while definitions use these providers.

`identity/0082_remove_package_index_approvals` (after `identity/0081_scheduled_executions`) deletes the Catalog Approvals of `core.npm` and `core.pypi` (#745). Its SQL is rendered by the Reinhardt Query builder in `identity::services::package_index_approvals` and checked by `manage migrationseeds --check`; reversal does not restore the approvals.

`execution/0133_egress_run_revocations` (after `execution/0132_generation_scheduled`) records revoked Runs and adds Run attribution and timing to scope connections. `0134_egress_run_revocations_rls` adds the key to the Tenant root and the forced Tenant policy; raw SQL is required because Reinhardt migrations do not express row-level policies. `execution/0135_agent_plugin_data` is database-only and creates `agent_plugin_data`, the saved `${PLUGIN_DATA}` of an Agent Plugin per Tenant, Scratch Space and Agent, with its Tenant key, owner key, bounds check and a cascading key to `workspaces` `scratch_spaces`; `0136_agent_plugin_data_rls` adds its forced Tenant policy in raw SQL for the same reason, and `0137_agent_plugin_data_state` is its state-only checkpoint.

`execution/0138_remove_legacy_egress_tokens` (after `execution/0137_agent_plugin_data_state`) drops the `egress_budgets` and `egress_sessions` tables, the `egress_requests.session_id` and `operation_id` columns and the `core_live_previews.egress_expires_at` column (#738): External services are reached only through the Execution Pod proxy, so operation tokens, per-operation budgets and Live preview session tokens are gone. `egress_requests.operation_id` and its key to `core_operations` go too: a record names only its Run, and Node purge follows that Run. `0139_remove_legacy_egress_tokens_state` updates the model state. Dependent keys, indexes and the `egress_counters` check are dropped with typed operations first, so reversal restores them; the table drops reverse to the historical schema with the original `created_at` default. Raw SQL is needed only for the two row-level policies, which Reinhardt migrations cannot express; the backward asset restores them after the tables return. Reversal restores empty tables.

`execution/0140_live_preview_unconfirmed` (after `execution/0139_remove_legacy_egress_tokens_state`) adds `core_live_previews.unconfirmed_since`. While a Node on the Run's chain does not answer the Read Recheck, the column holds since when, and the Preview gateway answers `503` instead of serving (#906, ADR 0129). It is a plain typed column addition with no raw SQL.

`identity/0083_data_protection_switches` follows ADR 0137 (#962, #963): it renames the Standard level to `no_restriction`, admits `safety_retention`, drops `model_call_zdr` and `plan_connection_exempt`, and adds the Tenant switches `forbid_plan_connections` and `forbid_keyless_endpoints` to the current policy and its history. Rewriting stored policies is data: both directions are Reinhardt Query output from `identity::services::data_protection_backfill`, materialized and checked by `migrationseeds`, and run in the Deployment Scope. A Tenant that never saved a policy read as Standard, so it first gets that policy as a row; it then becomes No Restriction like every other Standard Tenant instead of reading as a new Tenant's No Training (#963 decision 5). A Tenant that had turned the Plan Connection exemption off above Standard starts with Plan Connections forbidden. Rolling back maps Safety Retention to Zero Data Retention and keeps the added rows, which then hold the former default. `identity/0084_data_protection_switches_state` records the column changes in ORM state.

ADR 0138 (#987, #962) removes the Research category; page reading (`research.page@1`, operation `fetch`) stays as an ordinary tool. `execution/0141_remove_research` deletes the Web disclosure approval records (`core_records` of kind `disclosure`) together with their approval observations and the observation population rows of the removed `web_search` and `page_reading` kinds, which reports can no longer decode; those deletes are Reinhardt Query output from `execution::services::research_record_cleanup`, checked by `migrationseeds`. It then drops `research_attempts`, `research_sources`, `research_snapshots`, `research_fragments`, `research_originals`, `research_page_images`, `research_journal_results`, `research_revocations` and `research_account_terminations`. Their row policies, composite keys and partial indexes have no typed drop that reverses to the original schema, so the drop is SQL and the reverse SQL recreates the tables, empty, from the schema after `execution/0132`; the deleted rows and approval records are not restored. A Run still waiting on a Web disclosure approval cannot resume and needs cancelling. `execution/0142_remove_research_state` records the dropped tables in ORM state. `identity/0085_remove_research_approvals` deletes Approval Rules of the `web_search` and `page_reading` kinds, narrows `identity_approval_rules_values` to `tool_call`, `outbound` and `generation`, and deletes the Catalog Approvals of the removed definitions (Brave search, page find and view, every fixture operation and the Brave and fixture bundles); the deletes are Reinhardt Query output from `identity::services::research_approval_cleanup`, checked by `migrationseeds`. Rollback restores the wider check but not the deleted rows. `identity/0086_remove_tenant_retention_rules` drops `tenant_retention_rules` and `tenant_retention_rule_history`, which only set how long Document snapshots were kept; its reverse SQL recreates them, empty, and `identity/0087_remove_tenant_retention_rules_state` records the drop in ORM state. `knowledge/0056_journal_proofs_without_research` drops `memory_journal_proofs.research` and the part of `memory_journal_proofs_valid` that names it; rollback restores the column as an empty array. `knowledge/0057_journal_proofs_without_research_state` records it in ORM state. `registry/0074_remove_research_providers` wraps `aidash_descriptor_is_valid` so that among `research.*` providers it admits only `('research.page@1', 'fetch', 'research')`, keeping the earlier body as `aidash_descriptor_before_research_0070`, and removes draft Bindings to the removed definitions. That draft rewrite is the one handwritten DML of this change: keeping the remaining Bindings in order needs `jsonb_array_elements(...) WITH ORDINALITY` in FROM and `jsonb_agg(... ORDER BY ...)`, and Reinhardt Query has neither a set-returning table function in FROM nor an ordered aggregate. Following #952, the removed definitions keep their registry rows so pinned Agent versions still resolve; the registry checks are NOT VALID, so they are not rechecked. Page reading's definitions moved to 2.0.0 and need a fresh Catalog Approval; Catalog Approvals and Bindings of page reading 1.x stay and name the old definitions. Rollback restores the wider descriptor contract.

`identity/0088_approval_carry_over` (after `identity/0087_remove_tenant_retention_rules_state`) adds carry-over, scope and nullable source version and
approver columns to Catalog Approvals and their history. Existing rows are Node-scoped and do not
carry. `identity/0089_approval_carry_over_state` records the corresponding ORM state; the
physical `scope` default is retained for existing write paths.

`workspaces/0060_duty_template` (after `workspaces/0050_threads_state`) adds the nullable JSONB `duties.template` (in the ORM model): the Marketplace Duty template version a Duty was installed from (#1011 D4), its version key, Node, owner Tenant, package and version, and the install request's idempotency key, so a later version can be offered as an update and a retried install returns the same Duty. `workspaces/0061_duty_template_check` (database-only) checks that it is an object naming those five strings. Both use typed operations; existing Duties stay empty. Duty templates themselves are ordinary `marketplace_versions` rows of kind `duty_template`, so the Marketplace app has no migration.

`workspaces/0062_task_duty_script` (after `workspaces/0061_duty_template_check`) adds the database-only `tasks.duty_script` (boolean, default false), which a Duty sets on the Task it starts to run its Router or Watcher (#1011). The Task's `duty_script_runs` row goes with its Duty; the flag stays, so the harness fails that Task's Run instead of asking a model on its stub description. Deleting a Duty also cancels the Runs of its unsettled script Tasks. The ORM model never sets the column; its writer and reader use Reinhardt Query. `workspaces/0063_duty_deliveries` creates `duty_deliveries`, the webhook deliveries a Duty Trigger accepted, so a signed request sent again starts nothing: the SHA-256 of the body (which the signature covers), the sender's delivery id when it names one (`X-GitHub-Delivery`, `X-Aidash-Delivery` or `X-Request-Id`), the event the delivery recorded and when. Two unique keys, `(tenant, trigger_id, digest)` and a partial one on `(tenant, trigger_id, delivery_id)`, back the check the delivery makes under its Duty's row lock; a repeat is answered with the first event's receipt. Rows cascade with their Duty and trigger, and the scheduler pass forgets them after seven days (`duty_deliveries_received`). The table is written and read with Reinhardt Query and has no ORM model; `workspaces/0064_duty_deliveries_row_level_security` is the rendered Tenant-owned row-level policy asset. All three reverse cleanly.

`workspaces/0065_duty_pending_events` adds a Tenant-scoped outbox for Watcher output and Duty completion triggers (#1011). Pending events commit with script or tick settlement. The scheduler retries delivery and removes each row in the same transaction as the event receipt and any Task changes. Pending rows cascade with their receiving Duty or trigger. PostgreSQL row-level security uses SQL assets because Reinhardt cannot express row policies.

`workspaces/0066_deleted_duty_flow_ancestry` adds the database-only `tasks.deleted_duty_parent_id`. Deleting an Agent-created Duty copies its creating Task ID to the Tasks that outlive it. Flow traversal follows that link across Threads in both directions. Reversing the typed operations removes the preserved links and their index.
