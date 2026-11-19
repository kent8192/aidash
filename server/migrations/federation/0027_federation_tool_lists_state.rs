// reinhardt-migration-source: 1
//! Model state for `0026_federation_tool_lists` (#989).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0027_federation_tool_lists_state", "federation")
		.add_dependency("federation", "0026_federation_tool_lists")
		.state_only(true)
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("tool_request", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("tool_allowance", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("tool_block", FieldType::Jsonb).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("tool_request", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("tool_allowance", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("tool_block", FieldType::Jsonb).with_not_null(true),
			mysql_options: None,
		})
}
