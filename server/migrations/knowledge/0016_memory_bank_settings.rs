// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0016_memory_bank_settings", "knowledge")
        .add_dependency("knowledge", "0015_memory_purge")
        .add_operation(Operation::CreateTable {
            name: "memory_bank_settings".into(), columns: vec![
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("next_maintenance", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::ForeignKey { name: "memory_settings_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::ForeignKey { name: "memory_settings_provider".into(), columns: vec!["provider_id".into(),"provider_version".into()], referenced_table: "registry".into(), referenced_columns: vec!["id".into(),"version".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "memory_settings_revision".into(), expression: "revision > 0 AND revision < 9223372036854775807".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_bank_settings_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_bank_settings FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_bank_settings_atomic ON memory_bank_settings;".into()),
        }).atomic(true)
}
