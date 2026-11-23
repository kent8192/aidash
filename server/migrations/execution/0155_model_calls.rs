// reinhardt-migration-source: 1
//! Model Calls (#1068, ADR 0144) and owner links on Inference Attempts and Usage.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0155_model_calls", "execution")
		.add_dependency("execution", "0154_worker_stints")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "stints".into(),
			constraint: Constraint::Unique {
				name: "stints_tenant_run_id_key".into(),
				columns: vec!["tenant".into(), "run_id".into(), "id".into()],
			},
		})
		.add_operation(Operation::CreateTable {
			name: "model_calls".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid),
				ColumnDefinition::new("stint_id", FieldType::Uuid),
				ColumnDefinition::new("purpose", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text),
				ColumnDefinition::new("usage", FieldType::Jsonb),
				ColumnDefinition::new("started_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("ended_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::Unique {
					name: "model_calls_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
				Constraint::ForeignKey {
				name: "model_calls_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
				Constraint::ForeignKey {
				name: "model_calls_stint_fkey".into(),
				columns: vec!["tenant".into(), "run_id".into(), "stint_id".into()],
				referenced_table: "stints".into(),
				referenced_columns: vec!["tenant".into(), "run_id".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
				Constraint::Check { name: "model_calls_owner".into(), expression: "(run_id IS NULL) = (stint_id IS NULL)".into() },
				Constraint::Check { name: "model_calls_purpose".into(), expression: "purpose IN ('compaction_summary','memory_role','workbench_test','mcp_sampling')".into() },
				Constraint::Check { name: "model_calls_digest".into(), expression: "request_digest ~ '^sha256:[0-9a-f]{64}$'".into() },
				Constraint::Check { name: "model_calls_outcome".into(), expression: "(outcome IS NULL) = (ended_at IS NULL) AND (outcome IS NULL OR outcome IN ('accepted','failed','refused')) AND (ended_at IS NULL OR ended_at >= started_at)".into() },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("stint_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "inference_attempts".into(),
			column: ColumnDefinition::new("model_call_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_attempts".into(),
			constraint: Constraint::ForeignKey {
				name: "inference_attempts_model_call_fkey".into(),
				columns: vec!["tenant".into(), "model_call_id".into()],
				referenced_table: "model_calls".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_attempts".into(),
			constraint: Constraint::ForeignKey {
				name: "inference_attempts_stint_fkey".into(),
				columns: vec!["tenant".into(), "run_id".into(), "stint_id".into()],
				referenced_table: "stints".into(),
				referenced_columns: vec!["tenant".into(), "run_id".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_attempts".into(),
			constraint: Constraint::Check { name: "inference_attempts_at_most_one_owner".into(), expression: "stint_id IS NULL OR model_call_id IS NULL".into() },
		})
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("model_call_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::ForeignKey {
				name: "inference_usage_model_call_fkey".into(),
				columns: vec!["tenant".into(), "model_call_id".into()],
				referenced_table: "model_calls".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::ForeignKey {
				name: "inference_usage_stint_fkey".into(),
				columns: vec!["tenant".into(), "run_id".into(), "stint_id".into()],
				referenced_table: "stints".into(),
				referenced_columns: vec!["tenant".into(), "run_id".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::Check { name: "inference_usage_at_most_one_owner".into(), expression: "stint_id IS NULL OR model_call_id IS NULL".into() },
		})
		.add_operation(Operation::AddColumn {
			table: "context_compaction_attempts".into(),
			column: ColumnDefinition::new("model_call_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "context_compaction_attempts".into(),
			constraint: Constraint::ForeignKey {
				name: "context_compaction_attempts_model_call_fkey".into(),
				columns: vec!["tenant".into(), "model_call_id".into()],
				referenced_table: "model_calls".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::CreateIndex {
			table: "model_calls".into(),
			columns: vec!["tenant".into(), "run_id".into(), "started_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "model_calls".into(),
			columns: vec!["tenant".into(), "node".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "inference_attempts".into(),
			columns: vec!["tenant".into(), "model_call_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "inference_usage".into(),
			columns: vec!["tenant".into(), "model_call_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0155_model_calls_row_level_security.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0155_model_calls_row_level_security.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0155_model_calls.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0155_model_calls.sql").into()),
		})
}
