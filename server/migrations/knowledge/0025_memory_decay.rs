// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0025_memory_decay", "knowledge")
        .add_dependency("knowledge", "0024_openrouter_embeddings")
        .add_operation(Operation::CreateTable {
            name: "memory_unit_retention".into(),
            columns: vec![
                ColumnDefinition::new("unit_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("deliveries", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("last_delivered_at", FieldType::TimestampTz),
                ColumnDefinition::new("reactivated_at", FieldType::TimestampTz),
                ColumnDefinition::new("pinned", FieldType::Boolean).with_not_null(true),
                ColumnDefinition::new("dormant_policy", FieldType::Jsonb),
                ColumnDefinition::new("changed_at", FieldType::TimestampTz).with_not_null(true),
            ],
            constraints: vec![
                Constraint::ForeignKey { name: "retention_unit".into(), columns: vec!["unit_id".into()], referenced_table: "memory_units".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::ForeignKey { name: "retention_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "retention_deliveries".into(), expression: "deliveries >= 0 AND deliveries < 9223372036854775807".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "memory_bank_decay".into(),
            columns: vec![
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("policy", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("activated_at", FieldType::TimestampTz),
                ColumnDefinition::new("as_of", FieldType::TimestampTz),
                ColumnDefinition::new("cursor", FieldType::Uuid),
                ColumnDefinition::new("next_job", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![Constraint::ForeignKey { name: "decay_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger installation is not expressible by typed migration operations.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_unit_retention_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_unit_retention FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard(); CREATE TRIGGER memory_bank_decay_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_bank_decay FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_bank_decay_atomic ON memory_bank_decay; DROP TRIGGER memory_unit_retention_atomic ON memory_unit_retention;".into()),
        }).atomic(true)
}
