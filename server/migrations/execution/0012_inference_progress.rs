// reinhardt-migration-source: 1
//! Inference Attempts and their display-only progress, stored apart from the event journal.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0012_inference_progress", "execution")
		.database_only(true)
		.add_dependency("execution", "0011_binding_memory_merge")
		.add_operation(Operation::RunSQL { sql: include_str!("sql/forward/baseline_search_path.sql").into(), reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()) })
		.add_operation(Operation::CreateTable {
			name: "inference_attempts".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("usage_attempt", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("first_seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("last_seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("started_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("finished_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("pruned_at", FieldType::TimestampTz).with_not_null(false),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "inference_attempt_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
				Constraint::Check { name: "inference_attempt_outcome".into(), expression: "(outcome IS NULL AND reason IS NULL AND finished_at IS NULL AND pruned_at IS NULL) OR (outcome IN ('accepted', 'discarded') AND reason IS NULL AND finished_at IS NOT NULL) OR (outcome = 'interrupted' AND reason IN ('cancelled', 'lease_lost', 'stall', 'stream_error', 'revoked') AND finished_at IS NOT NULL)".into() },
				Constraint::Check { name: "inference_attempt_sequence".into(), expression: "first_seq >= 1 AND last_seq >= first_seq".into() },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// One provider call at a time: a Run has at most one pending attempt.
		.add_operation(Operation::CreateNamedIndex {
			table: "inference_attempts".into(),
			name: "inference_attempt_pending".into(),
			columns: vec!["run_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IS NULL)".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "inference_attempts".into(),
			name: "inference_attempt_run_sequence".into(),
			columns: vec!["run_id".into(), "first_seq".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "inference_attempts".into(),
			name: "inference_attempt_retention".into(),
			columns: vec!["finished_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IS NOT NULL AND pruned_at IS NULL)".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "inference_progress".into(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("item", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey { name: "inference_progress_pkey".into(), columns: vec!["run_id".into(), "seq".into()] },
				Constraint::ForeignKey { name: "inference_progress_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
				Constraint::ForeignKey { name: "inference_progress_attempt".into(), columns: vec!["attempt_id".into()], referenced_table: "inference_attempts".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
				Constraint::Check { name: "inference_progress_kind".into(), expression: "kind IN ('started', 'text', 'tool_call', 'outcome')".into() },
				Constraint::Check { name: "inference_progress_bounds".into(), expression: "seq >= 1 AND octet_length(item::text) <= 16384".into() },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "inference_progress".into(),
			name: "inference_progress_attempt".into(),
			columns: vec!["attempt_id".into(), "seq".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Statement trigger DDL is unsupported by the typed migration API.
		.add_operation(Operation::RunSQL { sql: include_str!("sql/forward/0012_inference_progress.sql").into(), reverse_sql: Some(include_str!("sql/backward/0012_inference_progress.sql").into()) })
		.atomic(true)
}
