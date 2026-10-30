// reinhardt-migration-source: 1
// Retain the provider container pin across cancellation, failure and worker recovery.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0082_provider_container", "execution")
		.add_dependency("execution", "0081_provider_tool_usage")
		.add_operation(Operation::AddColumn {
			table: "runs".into(),
			column: ColumnDefinition::new("provider_container", FieldType::Jsonb)
				.with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
