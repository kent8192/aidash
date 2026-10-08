// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0015_memory_purge", "knowledge")
        .add_dependency("knowledge", "0014_memory_review_audit")
        .add_operation(Operation::CreateTable {
            name: "memory_purge_jobs".into(), columns: vec![
                ColumnDefinition::new("unit_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("purge_after", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("backup_until", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("attempts", FieldType::Integer).with_not_null(true).with_default(Some("0".into())),
                ColumnDefinition::new("next_attempt", FieldType::TimestampTz).with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".into())),
                ColumnDefinition::new("last_error", FieldType::Text),
            ], constraints: vec![
                Constraint::ForeignKey { name: "memory_purge_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "memory_purge_state".into(), expression: "state IN ('pending','purged','failed') AND attempts >= 0 AND revision > 0 AND purge_after <= backup_until".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_purge_jobs_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_purge_jobs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_purge_jobs_atomic ON memory_purge_jobs;".into()),
        }).atomic(true)
}
