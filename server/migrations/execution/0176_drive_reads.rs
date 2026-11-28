// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0176_drive_reads", "execution")
		.atomic(true)
		.database_only(true)
		.add_dependency("execution", "0175_node_drives")
		.add_operation(Operation::CreateTable {
			name: "drive_run_reads".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![
				Constraint::Unique { name: "drive_run_reads_source_key".into(), columns: vec!["tenant".into(), "run_id".into(), "entry_id".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// Reinhardt has no operations for forced PostgreSQL row-level security.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0176_drive_reads.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0176_drive_reads.sql").into()),
		})
}
