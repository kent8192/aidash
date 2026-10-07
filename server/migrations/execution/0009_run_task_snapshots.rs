// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0009_run_task_snapshots", "execution")
        .add_dependency("execution", "0008_native_memory_usage")
        .add_operation(Operation::CreateTable {
            name: "run_task_snapshots".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("task_revision", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("step", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("input_seq", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("body", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("captured_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::ForeignKey { name: "run_task_snapshot_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "run_task_snapshot_bounds".into(), expression: "task_revision >= 0 AND step >= 0 AND input_seq >= 0".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL { sql: "CREATE TRIGGER run_task_snapshots_atomic BEFORE INSERT OR UPDATE OR DELETE ON run_task_snapshots FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(), reverse_sql: Some("DROP TRIGGER run_task_snapshots_atomic ON run_task_snapshots;".into()) })
        .atomic(true)
}
