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

Raw SQL is restricted to this frozen baseline's PostgreSQL schema DDL, including
functions, triggers, generated columns, exclusion/check constraints, and partial
indexes whose complete behavior cannot be represented by the pinned framework.
Business operations continue using native transactions and Query expressions.
Changing a model declaration does not authorize removing a corresponding database
constraint, trigger, lease fence, or visibility barrier.

## Commands

From the repository root, apply or inspect the native history:

```sh
cargo run --locked -p aidash-server --bin aidash -- migrate
cargo run --locked -p aidash-server --bin manage -- migrate --plan
cargo run --locked -p aidash-server --bin manage -- showmigrations
cargo run --locked -p aidash-server --bin manage -- makemigrations --state-source files --dry-run --check
```

Both migration entry points use the same graph and transaction-scoped advisory
lock `71003203` for the complete run. Initialization cannot use `--fake` or
`--fake-initial`. The compatibility `serve`, `server`, and `worker` commands apply
the same graph before assembling HTTP or worker dependencies.

The frozen baseline is forward-only. `manage migrate <app> zero` and an earlier
baseline target refuse execution, including `--fake`, before any native ledger
record changes. The physical SQL also refuses direct reversal. This protects the
schema and the state-only snapshots from Reinhardt's warn-and-skip behavior for
`RunSQL` without reverse SQL. `--plan` remains available for inspection. Subsequent
reversible migrations can be rolled back to the complete baseline. To undo the
initial cutover, stop all writers and restore the retained deployment/database;
never erase baseline ledger rows while leaving their tables in place.

For an intentional model change, run `manage makemigrations --state-source files`
from `server/` (or pass the server directory through the composed settings), inspect
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
