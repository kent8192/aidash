// reinhardt-migration-source: 1
// Remove the unused generated Summary Stage allowance and journal.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0127_remove_generated_summary", "execution")
		.add_dependency("execution", "0126_remove_generated_compaction_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0127_remove_generated_summary.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0127_remove_generated_summary.sql").into(),
			),
		})
		// Preserve the original default when reversing the table drop: the
		// historical model state records only its columns and primary key.
		.add_operation(Operation::AlterColumn {
			table: "generation_summary_usage".into(),
			column: "created_at".into(),
			old_definition: Some(ColumnDefinition::new("created_at", FieldType::TimestampTz)
				.with_not_null(true).with_default(Some("clock_timestamp()".into()))),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_budgets".into(),
			constraint: Constraint::Check {
				name: "generation_budgets_summary_calls_check".into(),
				expression:
					"((summary_calls >= 0) AND (summary_calls <= summary_call_limit))"
						.into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_budgets".into(),
			constraint: Constraint::Check {
				name: "generation_budgets_summary_call_limit_check".into(),
				expression: "(summary_call_limit >= 0)".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_policies".into(),
			constraint: Constraint::Check {
				name: "generation_policies_allocated_summary_calls_check".into(),
				expression: "(allocated_summary_calls >= 0)".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_summary_usage".into(),
			constraint: Constraint::Check {
				name: "generation_summary_usage_request_bytes_check".into(),
				expression: "(request_bytes > 0)".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_summary_usage".into(),
			constraint: Constraint::ForeignKey {
				name: "generation_summary_usage_request_id_fkey".into(),
				columns: vec!["tenant".into(), "request_id".into()],
				referenced_table: "generation_requests".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_summary_usage".into(),
			constraint: Constraint::ForeignKey {
				name: "generation_summary_usage_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropColumn {
			table: "generation_budgets".into(),
			column: "summary_call_limit".into(),
			old_definition: Some(
				ColumnDefinition::new("summary_call_limit", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "generation_budgets".into(),
			column: "summary_calls".into(),
			old_definition: Some(
				ColumnDefinition::new("summary_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "generation_policies".into(),
			column: "allocated_summary_calls".into(),
			old_definition: Some(
				ColumnDefinition::new("allocated_summary_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression:
					"purpose IN ('embedding', 'inference', 'memory', 'summary')"
						.into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression: "purpose IN ('embedding', 'inference', 'memory')".into(),
			},
		})
		// Typed reversal treats the historical composite key as two individual
		// primary keys; use DDL to recreate the original key on rollback.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0127_drop_summary_usage.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0127_drop_summary_usage.sql").into()),
		})
}
