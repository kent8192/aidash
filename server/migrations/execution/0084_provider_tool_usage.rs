// reinhardt-migration-source: 1
// Preserve provider-reported tool counts separately from estimates and token usage.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0084_provider_tool_usage", "execution")
		.add_dependency("execution", "0083_harness_contract_state")
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("server_tool_use", FieldType::Jsonb),
			mysql_options: None,
		})
		.atomic(true)
}
