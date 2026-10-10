// reinhardt-migration-source: 1
//! Generated Summary Stage allowances: per-agent call budgets, policy
//! allocation, durable per-attempt usage and the remote `summary` purpose.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0014_summary_allowance", "execution")
		.database_only(true)
		.add_dependency("execution", "0013_context_journal_model_state")
		.add_operation(Operation::AddColumn {
			table: "generation_budgets".into(),
			column: ColumnDefinition::new("summary_call_limit", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "generation_budgets".into(),
			column: ColumnDefinition::new("summary_calls", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_budgets".into(),
			constraint: Constraint::Check {
				name: "generation_budgets_summary_call_limit_check".into(),
				expression: "(summary_call_limit >= 0)".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_budgets".into(),
			constraint: Constraint::Check {
				name: "generation_budgets_summary_calls_check".into(),
				expression: "((summary_calls >= 0) AND (summary_calls <= summary_call_limit))"
					.into(),
			},
		})
		.add_operation(Operation::AddColumn {
			table: "generation_policies".into(),
			column: ColumnDefinition::new("allocated_summary_calls", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policies".into(),
			constraint: Constraint::Check {
				name: "generation_policies_allocated_summary_calls_check".into(),
				expression: "(allocated_summary_calls >= 0)".into(),
			},
		})
		.add_operation(Operation::CreateTable {
			name: "generation_summary_usage".into(),
			columns: vec![
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("definition_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("clock_timestamp()".into())),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "generation_summary_usage_pkey".into(),
					columns: vec!["request_id".into(), "attempt_id".into()],
				},
				Constraint::Check {
					name: "generation_summary_usage_request_bytes_check".into(),
					expression: "(request_bytes > 0)".into(),
				},
				Constraint::ForeignKey {
					name: "generation_summary_usage_request_id_fkey".into(),
					columns: vec!["request_id".into()],
					referenced_table: "generation_requests".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "generation_summary_usage_run_id_fkey".into(),
					columns: vec!["run_id".into()],
					referenced_table: "runs".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "generation_summary_usage_provider_id_provider_version_fkey".into(),
					columns: vec!["provider_id".into(), "provider_version".into()],
					referenced_table: "registry".into(),
					referenced_columns: vec!["id".into(), "version".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// Statement trigger DDL is unsupported by the typed migration API.
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON generation_summary_usage FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
			reverse_sql: Some("DROP TRIGGER atomic_write_guard ON generation_summary_usage;".into()),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression: "purpose IN ('embedding', 'inference', 'compaction', 'memory')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression: "purpose IN ('embedding', 'inference', 'compaction', 'memory', 'summary')"
					.into(),
			},
		})
		.atomic(true)
}
