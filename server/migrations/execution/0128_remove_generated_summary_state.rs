// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0128_remove_generated_summary_state", "execution")
		.add_dependency("execution", "0127_remove_generated_summary")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "generation_summary_usage".into(),
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
}
