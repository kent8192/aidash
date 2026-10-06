// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0022_memory_receiver_caches", "knowledge")
        .add_dependency("knowledge", "0021_memory_unit_run_origins")
        .add_operation(Operation::CreateTable {
            name: "memory_receiver_caches".into(),
            columns: vec![
                ColumnDefinition::new("run_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("invalidated", FieldType::Boolean).with_not_null(true),
                ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("attempts", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("max_attempts", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("next_attempt", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("last_error", FieldType::Text),
            ],
            constraints: vec![
                Constraint::ForeignKey { name: "memory_receiver_cache_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "memory_receiver_cache_state".into(), expression: "state IN ('pending','purged','failed') AND attempts >= 0 AND max_attempts > 0 AND attempts <= max_attempts".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_receiver_caches_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_receiver_caches FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_receiver_caches_atomic ON memory_receiver_caches;".into()),
        }).atomic(true)
}
