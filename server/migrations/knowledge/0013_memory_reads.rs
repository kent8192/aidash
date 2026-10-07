// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0013_memory_reads", "knowledge")
        .add_dependency("knowledge", "0012_discard_json_banks")
        .add_operation(Operation::CreateTable {
            name: "memory_run_reads".into(),
            columns: vec![
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("unit_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
            ],
            constraints: vec![
                Constraint::PrimaryKey { name: "memory_run_reads_pkey".into(), columns: vec!["run_id".into(), "unit_id".into(), "revision".into()] },
                Constraint::Check { name: "memory_read_revision".into(), expression: "revision > 0".into() },
                Constraint::ForeignKey { name: "memory_read_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::ForeignKey { name: "memory_read_unit".into(), columns: vec!["unit_id".into()], referenced_table: "memory_units".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "memory_run_read_gates".into(),
            columns: vec![ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true).with_primary_key(true)],
            constraints: vec![Constraint::ForeignKey { name: "memory_read_gate_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_run_reads_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_run_reads FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard(); CREATE TRIGGER memory_run_read_gates_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_run_read_gates FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_run_read_gates_atomic ON memory_run_read_gates; DROP TRIGGER memory_run_reads_atomic ON memory_run_reads;".into()),
        })
        .atomic(true)
}
