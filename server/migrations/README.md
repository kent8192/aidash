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
ownership before this new native history is published. The complete native graph contains 46
records and describes 117 models.

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
`--fake-initial`. The compatibility `serve`, `server`, and `worker` commands apply
the same graph before assembling HTTP or worker dependencies.

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

## Maintenance cutover

1. Stop new writes and request graceful shutdown of every old server and worker.
   Wait for drain, then back up their database and retain the old binaries.
2. Provision a separate empty PostgreSQL 17 database with the `pg_jsonschema` 0.3.4
   library available. A superuser may let native history create the extension.
   For a database-scoped application role, provision `pg_jsonschema` as the
   administrator first; the application must not receive superuser privileges.
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

## Physical schema verification

`test-migration-schema.py` compares the shared migration bootstrap on a unique empty
`template0` database with a read-only PostgreSQL reference built from the immutable
native revision `cd29635a9937133d2e81cca44857bea331446cc5`. This reference includes
native memory, vector, and PGroonga additions. Prepare it with `aidash migrate`
using binaries, settings, and migration sources from that exact revision in an
isolated deployment directory; do not rebuild the reference from the working tree.
The reference and target must each have exactly the `(app, name)` identities
registered in their respective source histories, with no retired ledger. New
migration files are included automatically without a hard-coded record count.

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
state, and executable hashes under `.ignore/schema-parity/`. Intentional physical
schema changes require a separately reviewed immutable reference update.

The native memory additions end with generated state-only ORM metadata checkpoints
for execution, knowledge, and registry. These reconcile composite-key and field
metadata and exclude procedural checks and references from the ORM snapshot, while
retaining the physical constraints established by the preceding migrations.
