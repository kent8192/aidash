// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0018_memory_control_receipts", "knowledge")
        .add_dependency("knowledge", "0017_memory_engine_jobs")
        .add_operation(Operation::CreateTable {
            name: "memory_learning_discovery".into(),
            columns: vec![
                ColumnDefinition::new("home", FieldType::Text).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("cursor_updated", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("cursor_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("cycle_before", FieldType::TimestampTz).with_not_null(true),
            ],
            constraints: vec![], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "memory_bank_policy_changes".into(),
            columns: vec![
                ColumnDefinition::new("operation_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("applied_revision", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("actor", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
            ],
            constraints: vec![
                Constraint::ForeignKey { name: "memory_policy_change_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "memory_policy_change_revision".into(), expression: "applied_revision > 0 AND applied_revision < 9223372036854775807".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_learning_discovery_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_learning_discovery FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard(); CREATE TRIGGER memory_bank_policy_changes_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_bank_policy_changes FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_bank_policy_changes_atomic ON memory_bank_policy_changes; DROP TRIGGER memory_learning_discovery_atomic ON memory_learning_discovery;".into()),
        }).atomic(true)
}
