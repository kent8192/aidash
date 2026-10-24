// reinhardt-migration-source: 1
// Research-owned exact Context Journal variants (#331). The append-only
// journal contains references; these nullable bytes follow Research purges.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_research_journal_results", "execution")
        .add_dependency("execution", "0049_foreign_key_relations_state")
        .database_only(true)
        .atomic(true)
        .add_operation(Operation::CreateTable {
            name: "research_journal_results".into(), columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true).with_primary_key(false),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true).with_primary_key(false),
                ColumnDefinition::new("invocation_key", FieldType::Text).with_not_null(true).with_primary_key(false),
                ColumnDefinition::new("digest", FieldType::Text).with_not_null(true).with_primary_key(false),
                ColumnDefinition::new("reference", FieldType::Jsonb).with_not_null(true).with_primary_key(false),
                ColumnDefinition::new("bytes", FieldType::Bytea).with_not_null(false).with_primary_key(false),
                ColumnDefinition::new("purge_reason", FieldType::Text).with_not_null(false).with_primary_key(false),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_primary_key(false),
            ], constraints: vec![
                Constraint::ForeignKey { name: "research_journal_results_tenant_root_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
                Constraint::ForeignKey { name: "research_journal_results_run".into(), columns: vec!["tenant".into(), "run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Unique { name: "research_journal_results_variant".into(), columns: vec!["run_id".into(), "invocation_key".into(), "digest".into()] },
                Constraint::Check { name: "research_journal_results_payload".into(), expression: "(bytes IS NULL OR (octet_length(bytes) BETWEEN 1 AND 1048576 AND purge_reason IS NULL)) AND (purge_reason IS NULL OR purge_reason IN ('account_terms', 'revocation')) AND digest ~ '^sha256:[a-f0-9]{64}$'".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
}
