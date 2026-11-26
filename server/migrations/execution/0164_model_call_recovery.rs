// reinhardt-migration-source: 1
//! Model Call retry identity and durable auxiliary deadlines (#1068).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0164_model_call_recovery", "execution")
		.add_dependency("execution", "0163_scope_host_debit_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "model_calls".into(),
			column: ColumnDefinition::new("retry_key", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("recovery_after", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("late_report_pending", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})

		.add_operation(Operation::CreateIndex {
			table: "model_calls".into(),
			columns: vec!["tenant".into(), "run_id".into(), "purpose".into(), "retry_key".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("retry_key IS NOT NULL AND run_id IS NOT NULL AND purpose IN ('compaction_summary','mcp_sampling')".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "inference_usage".into(),
			columns: vec!["recovery_after".into(), "attempt_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("purpose = 'auxiliary' AND outcome = 'dispatched' AND model_call_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
