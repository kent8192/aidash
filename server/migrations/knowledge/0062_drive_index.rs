// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0062_drive_index", "knowledge")
		.atomic(true)
		.database_only(true)
		.add_dependency("knowledge", "0059_threads_state")
		.add_dependency("execution", "0175_node_drives")
		.add_operation(Operation::CreateTable {
			name: "drive_index_requests".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("version", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("requester", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("estimated_micros", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_index_requests_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_index_requests_version_key".into(), columns: vec!["drive_id".into(), "version".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_passages".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("embedding_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("ordinal", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("start_line", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("end_line", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("body", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("vector", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![
				Constraint::Unique { name: "drive_passages_content_key".into(), columns: vec!["tenant".into(), "digest".into(), "embedding_key".into(), "ordinal".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_index_requests".into(), name: "drive_index_pending".into(),
			columns: vec!["tenant".into(), "state".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		// Reinhardt has no operations for forced PostgreSQL row-level security.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0062_drive_index.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0062_drive_index.sql").into()),
		})
}
