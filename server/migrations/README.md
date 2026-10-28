# Aidash native migration history

Issue #296 adds write-only Tenant Secrets in `identity/0036` through `0038`
and durable operation egress budgets and audits in `execution/0046` through
`0048`. Their forced Tenant row policies use the existing policy renderer.
`registry/0042_external_services` extends both Registry kind checks and the
HTTP/MCP and Agent Binding contracts; `0043` records the ORM enum state. Kind
constraints use typed Reinhardt operations. PostgreSQL function replacements
remain SQL DDL assets because Reinhardt has no function-body operation. The
reverse restores the previous Tool and Agent contracts; remove External
service definitions and bindings before reversing the kind constraints.

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
ownership before this new native history is published. The complete native graph contains 57
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

The two baseline seed mutations and their inverses are built with Reinhardt Query
in the owning federation and marketplace apps. `manage migrationseeds --check`
compares their rendered PostgreSQL statements with the four canonical SQL assets.
`manage migrationseeds --write` regenerates only those assets without reading
runtime credentials or opening a database. Regeneration is for an **unapplied**
history; never rewrite an applied migration. The native filesystem source parses
static Rust syntax without executing arbitrary function calls, so these generated
assets remain literal `include_str!` payloads in the native graph. This is
materialized Query output, not a handwritten DML exception or a second migration
engine. The sole raw statement in the forward seed assets is the transaction-local
search-path setting. Regression checks load the exact generated SQL through
`FilesystemSource` and verify both initial gate values after migration and after
complete reversal/reapplication.

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

`execution/0060_plugin_research_merge` and `registry/0049_plugin_context_merge`
join the MCP and Research/approval histories, and the Agent Plugin and Context
Source histories, respectively. Both are empty state-only migrations. They
preserve the existing migration identities and give each app one leaf without
changing schema, retained inputs, or data.

`execution/0080_invocation_tenant_secrets` (after `execution/0079_run_safety_stops_state`)
adds the nullable `tenant_secret_id`, `tenant_secret_version` and
`tenant_secret_host` of `invocations` with typed column operations (#310, ADR
0095). A tool call without a Tenant Secret leaves them null; a call that carried
one records which Tenant Secret, at which pinned version, went to which host,
never its value.

`execution/0081_egress_secret_records` (after `execution/0080_invocation_tenant_secrets`)
adds the nullable `secret_id`, `secret_version` and `secret_host` of
`egress_requests` with typed column operations (#310, ADR 0095). A request for
an External service without a Tenant Secret leaves them null; a brokered request
records the Tenant Secret, the pinned version its token names and the
destination host, never the value or a header value. The typed operations update
the ORM snapshot, so no state-only checkpoint is needed.

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

`execution/0082_invocation_connections` (after `execution/0081_egress_secret_records`)
adds nullable `invocations.connection_id` with a typed Reinhardt operation (#554,
ADR 0100). It records the person's OAuth Connection used by dispatch, never a token.
