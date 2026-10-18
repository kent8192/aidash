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
  Tenant-owned (enforced, or pending the Issue that enforces it) or
  Deployment-wide. The `startup` test target compares the list with the
  migrated database, so a migration that adds a table classifies it in the same
  change.

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
state, and executable hashes under `.ignore/schema-parity/`. Intentional physical
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

`registry/0032_research_view` (after `registry/0031_model_request_estimator`, and like it after the Tenant Registry migrations of #243) admits the research `view` operation for `research.page@1` and `research.fixture@1` in `aidash_descriptor_is_valid` (#312). The function body has no typed Reinhardt operation, so it is a reversible SQL asset; it extends the `0023_research_providers` body and rollback restores that body. It does not delete descriptors that already declare `view`, whose later rewrites the NOT VALID registry checks then reject.

`workspaces/0008_publishing_run` (after `workspaces/0007_model_state`) adds a nullable `run_id` to `messages` and `artifacts` with typed `AddColumn` operations. It records the Run that published the text on this Node, so a Source citation in it resolves only against that Run's Research evidence. It has no foreign key: the value outlives a deleted Run, so such a citation reads as unavailable rather than as text nobody published. Existing rows keep `NULL`, and rollback drops both columns.

`execution/0029_research_retention` (after `execution/0028_url_intents_model_state`) adds `purged_at` and `purge_reason` to `research_snapshots` with typed operations. A check keeps both set or both null, limits the reason to `retention` or `revocation`, and requires a Purged snapshot to keep no contents (`lines = '[]'`, `text_bytes = 0`). A partial index on unpurged snapshot ids serves the retention sweep. It also creates `research_revocations`, keyed by Run with no reference to `runs`, so the audit record of an Evidence Revocation outlives its Run (ADR 0046), with a `(tenant, revoked_at)` index for listing. The generated state-only checkpoint `execution/0030_research_retention_state` records the columns and table in the ORM snapshot without the checks and indexes.

`execution/0031_research_originals` (after `execution/0030_research_retention_state`) records, for a snapshot of a paginated source, its page count, the pages that carry no text, and the digest and size of its original, with typed `AddColumn` operations (#312, ADR 0053). A check keeps the digest and size set together, bounds the size to 16 MiB and allows `original_retained` only while the snapshot is not purged. It creates `research_originals`, keyed by snapshot with a cascading reference to `(run_id, id)` of `research_snapshots`, so that reading a snapshot never loads its original; a purge or an Evidence Revocation deletes the rows, and a `run_id` index serves the per-Run limit. The generated state-only checkpoint `execution/0032_research_originals_state` records the columns and table without the checks and index.

`execution/0033_research_event_run_index` (after `execution/0032_research_originals_state`) adds the partial expression index `events_tool_run` on `(data ->> 'run_id')` for `tool.started` and `tool.completed` events, which Evidence Revocation and the retention sweep redact per Run. It changes no model state, so it has no state-only checkpoint.

`execution/0034_tokenized_estimate_confidence` (after `execution/0033_research_event_run_index`) replaces the `inference_usage_estimate_confidence` check with typed constraint operations, so that a Usage Record may carry the `tokenized` confidence of a Token Encoding (#276, ADR 0017) and no longer the unused `exact`. The check has been enforced apart from the ORM state since `execution/0013_inference_usage_model_state`, so the migration is database-only and has no state-only checkpoint.

`execution/0035_research_page_images` (after `execution/0034_tokenized_estimate_confidence`) adds the page image of an Evidence fragment (`image_page`, `image_media_type`, `image_width`, `image_height`, `image_digest`) to `research_fragments` with typed operations (#312, ADR 0053). It replaces the `research_fragments_lines` check so that a fragment is either a passage (no image columns, `line_start >= 1`) or one PNG page image of at most 1568 pixels per edge with no lines. It creates `research_page_images`, keyed by fragment with a cascading reference to `research_fragments`, at most 1 MiB per image, so that reading fragments never loads image bytes; an Evidence Revocation deletes the rows, and a `run_id` index serves the per-Run view limit. The generated state-only checkpoint `execution/0036_research_page_images_state` records the columns and table without the checks and index.

`identity/0020_tenancy_schema` (after `identity/0019_provider_authorizations`) creates the Tenant root records (`tenants`, ADR 0050), `personal_tenants` and `tenant_invitations` (indexed by Tenant with `expires_at` and with `created_at`) with typed operations, adds the addressing `tenant` and `invitation_id` columns to `dashboard_registration_requests` (indexed with `status` and `expires_at` for one Tenant's pending requests) and `invitation_id` to `dashboard_administration_history`, and references `authorization_bundles.tenant` to its root record. A reversible SQL asset backfills a root record for every existing policy bundle, and a Personal Tenant link for every `personal-<identity>` Tenant, before the reference is added. Making `dashboard_administration_history.identity_id` nullable is the one `ALTER TABLE` statement in `RunSQL`: the pinned typed `AlterColumn` renders no `NOT NULL` change for PostgreSQL ([#6803](https://github.com/kent8192/reinhardt-web/issues/6803)). The state-only checkpoint `identity/0021_tenancy_state` records the matching models.

`identity/0024_tenant_retention_rules` (after `identity/0023_plan_connection_constraints`) creates `tenant_retention_rules`, one row per Tenant that keeps its own snapshot periods, and `tenant_retention_rule_history`, one row per change, where a cleared change has no periods (ADR 0045). Checks bound every period to ten years. The tables carry a `tenant` column; their row-level policies come with RLS step 3 (#264). The generated state-only checkpoint `identity/0025_tenant_retention_rules_state` records both tables in the ORM snapshot without the checks and index.
