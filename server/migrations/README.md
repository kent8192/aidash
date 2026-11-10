# Aidash native migration history

Issue #861 rebaselined the native history at `268497a929bf5e32cc866939fef6b5e134950793`.
The old sources remain in [Git](https://github.com/kent8192/aidash/tree/268497a929bf5e32cc866939fef6b5e134950793/server/migrations).
Each app has a typed state-only snapshot, physical table baseline, and a
cross-app references baseline. Physical SQL is captured from PostgreSQL;
the baseline is irreversible. `operations/0000_environment` remains first.

## Supported databases

The baseline accepts a new, empty PostgreSQL database. Existing databases from
the old history, including staging and local Compose databases, must be
recreated. Unknown migration records are rejected at startup; there is no
in-place adoption path.

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

The rebaselined physical migrations are irreversible. Recreate the database
instead of reversing or faking the baseline. The bootstrap refuses unknown or
inconsistent recorded histories.

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

- `identity/0002_tables` creates both functions. They are
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

- `identity/0002_tables` also creates `aidash_tenant_purge()`,
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
- Each app's `0003_references` enables its Tenant policies after every app's
  tables exist; `0002_tables` retains the forced-row-security flags. PostgreSQL
  policy DDL is captured in SQL because Reinhardt has no policy operation.
  The policy tests compare the captured definitions and table classification.
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

## Tenant and Node references

Tenant-owned tables carry `tenant` and reference the Tenant root. Child-to-parent
foreign keys include the Tenant where both rows are Tenant-owned. `nodes` is
keyed by `(tenant, name)`; Node-owned data carries `(tenant, node)` and its
foreign keys preserve that ownership. `node_names` is Deployment-wide and keeps
its name claim after a Tenant purge. The database classification and tenancy
tests check these boundaries against the migrated schema.

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

To regenerate against a later old history and compare fresh databases:

```sh
python3 scripts/rebaseline-migrations.py --manage /absolute/path/to/manage --old-ref <old-ref> --generate
python3 scripts/rebaseline-migrations.py --manage /absolute/path/to/manage --old-ref <old-ref> --verify
```

The tool uses an isolated disposable PostgreSQL container and records raw and
normalized dumps and diffs under `private/rebaseline/`. It normalizes PostgreSQL
CHECK-expression rendering by restoring both schema dumps before comparing them.
The seed-data comparison excludes the migration ledger and its sequence.
