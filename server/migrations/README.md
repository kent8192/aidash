# Aidash native migration history

Reinhardt is the sole migration owner. The source files are grouped under the
labels `identity`, `registry`, `workspaces`, `execution`, `federation`, `knowledge`,
`marketplace`, and `operations`; these are modules in `aidash-server`, not separate
Cargo packages.

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
monolithic Rust source are removed from this workspace. [baseline.json](baseline.json) lists the 54 legacy migration names,
607 schema objects, and their native app ownership. This is an invariant map;
it does not translate or import the old migration ledger.

There are 36 migrations for physical schema creation and eight `0007_model_state`
snapshots. The physical graph orders functions, tables, keys/indexes, references,
seed data, and triggers across app boundaries. The snapshots describe 115 ORM
models for subsequent autodetection and are **state-only**: they do not recreate
tables or replace procedural constraints.

Physical table definitions, named primary/unique/foreign keys, CHECK constraints,
generated columns, named ordinary/expression/partial indexes, column defaults, and
the extension use `reinhardt::db::migrations::Operation`. SQL expressions in CHECKs,
defaults, index predicates, and generated-column metadata are expression bodies;
whole supported CREATE/ALTER statements must not be passed to `RunSQL`.

Operations absent from the pinned migration API are stored in each app's `sql/forward/`
directory and loaded with `include_str!` into `Operation::RunSQL`: procedural
functions/triggers/DO blocks, explicit sequences and ALWAYS identity options,
transaction-local session settings, and the two baseline seed inserts. Every
physical migration sets `search_path` to `public, pg_catalog` locally because the
pinned typed operations accept unqualified names; this preserves the frozen
public schema even with a custom connection search path.
Corresponding reverse SQL assets live in `sql/backward/` and are included in
`RunSQL.reverse_sql`. Independent function/trigger definitions in one migration
share a single SQL asset and operation; sequence/table dependency boundaries remain
ordered. Independent sequences are created together before their tables; column
defaults are part of typed table definitions, and ownership/identity configuration
is grouped after the tables. This avoids the pinned `AlterColumn` inverse's missing
default restoration. Reverse scripts drop triggers/functions in reverse order,
remove identity configuration, detach/drop explicit sequences, and delete only
baseline seed rows.
Transaction-local settings have no persistent inverse; their backward files use
an explicit no-op DO block. The environment's backward asset drops the extension
because the pinned `CreateExtension` variant has no native inverse. A final
context operation sets the public search path before typed reverse operations run.

Reinhardt Query already provides builders for several of these statements; the
remaining gap is their integration into migration operations and filesystem
source loading. Retain the procedural constraints, lease fences, and visibility
barriers when changing a model.

Migration sources and SQL assets use LF line endings. Do not embed NEL, LS, PS,
vertical tab, form feed, or carriage return characters. PostgreSQL escape string
literals preserve the existing Unicode whitespace set in nonblank CHECKs
without putting special line terminators in source files.

The native `FilesystemSource` resolves literal `include_str!` references within
confined sibling SQL assets. Execution, inspection, and `makemigrations` use the
original app history directly, without a project-specific expansion or child
command adapter.

Upstream SQL asset support [#6505](https://github.com/kent8192/reinhardt-web/issues/6505)
is included in the pinned revision `e43a0194d40155c9e18afac39c68ce92f3b9d3c7`.
PostgreSQL sequence/identity migration operations and Query-backed procedural,
session, and seed operations remain tracked in
[#6506](https://github.com/kent8192/reinhardt-web/issues/6506) and
[#6507](https://github.com/kent8192/reinhardt-web/issues/6507).
Cross-app reverse planning and missing default/extension inverses are tracked in
[#6515](https://github.com/kent8192/reinhardt-web/issues/6515) and
[#6516](https://github.com/kent8192/reinhardt-web/issues/6516).

## Commands

From the repository root, apply or inspect the native history:

```sh
cargo run --locked -p aidash-server --bin aidash -- migrate
cargo run --locked -p aidash-server --bin manage -- migrate --plan
cargo run --locked -p aidash-server --bin manage -- showmigrations
cargo run --locked -p aidash-server --bin manage -- makemigrations --state-source files --migration-dir server/migrations
cargo run --locked -p aidash-server --bin manage -- makemigrations --state-source files --migration-dir server/migrations --dry-run --check
```

Both migration entry points use the same graph and transaction-scoped advisory
lock `71003203` for the complete run. Initialization cannot use `--fake` or
`--fake-initial`. The compatibility `serve`, `server`, and `worker` commands apply
the same graph before assembling HTTP or worker dependencies.

The physical baseline is reversible. The local backward planner expands applied
dependents through Reinhardt's complete graph, orders them with its topological
sort, and uses its native executor to reverse each migration atomically. The
pinned command otherwise considers only the selected app. Remove the planner
when the upstream command provides cross-app reversal and the baseline tests pass.
`manage migrate operations zero` reverses the complete baseline, including model
state, functions, triggers, schema objects, seed data, and extension creation.
Applying the baseline again recreates an empty schema. Partial app targets use
the same dependency graph. `--fake` baseline reversal remains prohibited because
it would remove ledger records while retaining physical objects. `--plan` and
`sqlmigrate --backwards` remain available for inspection. Existing SeaORM history
still cannot be adopted by this native ledger.

For an intentional model change, run `manage makemigrations --state-source files`
from `server/` (or explicitly pass `--migration-dir server/migrations` from the workspace root), inspect
the generated app migration, and retain any procedural DDL required by the business
invariants. Use one history throughout; do not run the retired SeaORM migrator.
`temporary-db` is inappropriate for this baseline because a generic PostgreSQL
image does not provide its required `pg_jsonschema` extension.

## Maintenance cutover

1. Stop new writes and request graceful shutdown of every old server and worker.
   Wait for drain, then back up their database and retain the old binaries.
2. Provision a separate empty PostgreSQL 17 database with `pg_jsonschema` 0.3.4.
   Configure all new replicas to use that database and the same pinned image.
3. Run the native migration command once. Inspect `showmigrations` and verify
   replay, schema constraints, and configured peer/identity settings before
   accepting writes.
4. Start the new server and worker roles. Verify health, authentication, a scoped
   workflow, SSE reconnection, and the configured Federation peers.

This cutover starts a new database; it does not transfer existing workspaces,
identities, credentials, or active Runs. Keep the old deployment/database intact
for rollback. Stop all new writers before restoring the old configuration.
Never let old and new migration engines manage the same application schema.
