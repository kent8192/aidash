// reinhardt-migration-source: 1
// The physical default serves raw inserts; ordered and partial indexes keep
// each read's candidate scan aligned with its Task, Run and response filter.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0085_bounded_read_indexes", "execution")
		.add_dependency("execution", "0084_run_created_at")
		.add_operation(Operation::AlterColumn {
			table: "runs".into(),
			column: "created_at".into(),
			old_definition: Some(
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz)
				.with_not_null(true)
				.with_default(Some("now()".into())),
			mysql_options: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "runs".into(),
			name: "runs_task_created_page".into(),
			columns: vec![
				"tenant".into(),
				"task_id".into(),
				"created_at".into(),
				"id".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_created_page".into(),
			columns: vec!["tenant".into(), "created_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_run_created_page".into(),
			columns: vec![
				"tenant".into(),
				"run_id".into(),
				"created_at".into(),
				"id".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_pending_created_page".into(),
			columns: vec!["tenant".into(), "created_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("response IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_answered_created_page".into(),
			columns: vec!["tenant".into(), "created_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("response IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_run_pending_created_page".into(),
			columns: vec![
				"tenant".into(),
				"run_id".into(),
				"created_at".into(),
				"id".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("response IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "human_requests".into(),
			name: "human_run_answered_created_page".into(),
			columns: vec![
				"tenant".into(),
				"run_id".into(),
				"created_at".into(),
				"id".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("response IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.atomic(true)
		.database_only(true)
}
