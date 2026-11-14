// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0126_remove_generated_compaction_state", "execution")
		.add_dependency("execution", "0125_remove_generated_compaction")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "generation_compaction_usage".into(),
		})
		.add_operation(Operation::DropColumn {
			table: "generation_budgets".into(),
			column: "compaction_call_limit".into(),
			old_definition: Some(
				ColumnDefinition::new("compaction_call_limit", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "generation_budgets".into(),
			column: "compaction_calls".into(),
			old_definition: Some(
				ColumnDefinition::new("compaction_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "generation_policies".into(),
			column: "allocated_compaction_calls".into(),
			old_definition: Some(
				ColumnDefinition::new("allocated_compaction_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			),
		})
}
