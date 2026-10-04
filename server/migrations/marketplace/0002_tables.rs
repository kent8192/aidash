// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "marketplace")
        .add_dependency("knowledge", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_audiences (
    key text NOT NULL,
    document jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_consents (
    key text NOT NULL,
    document jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_gate (
    key text NOT NULL,
    document jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_installations (
    key text NOT NULL,
    document jsonb NOT NULL,
    tenant text NOT NULL,
    package_key text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_provenance (
    key text NOT NULL,
    document jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_requests (
    key text NOT NULL,
    document jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_revisions (
    key text NOT NULL,
    document jsonb NOT NULL,
    installation text NOT NULL,
    revision bigint NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.marketplace_versions (
    key text NOT NULL,
    document jsonb NOT NULL,
    repository text NOT NULL,
    owner text NOT NULL,
    package_id text NOT NULL,
    version text NOT NULL,
    kind text NOT NULL,
    source_id text NOT NULL,
    source_version text NOT NULL,
    source_content text NOT NULL
);"#.to_string(),
            // Refuse irreversible baseline rollback before the native ledger changes.
            reverse_sql: Some(r#"DO $aidash_baseline$
BEGIN
    RAISE EXCEPTION 'Aidash frozen baseline is forward-only; restore a backup to roll back';
END
$aidash_baseline$;"#.to_string()),
        })
        .atomic(true)
        .database_only(true)
}
