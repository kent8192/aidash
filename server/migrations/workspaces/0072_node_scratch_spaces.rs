// reinhardt-migration-source: 1
// Allocate a shared space on every Node and replace the pre-release bindings.
// Old space data is discarded. The historical rows did not record whether
// a person created a space by name or the system created it for a Thread.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0072_node_scratch_spaces", "workspaces")
		.add_dependency("workspaces", "0071_completion_messages")
		.add_dependency("execution", "0135_agent_plugin_data")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "scratch_spaces".into(),
			column: ColumnDefinition::new("is_default", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0072_node_scratch_spaces.sql").into(),
			// Reversal restores the schema. Retired contents are disposable.
			reverse_sql: Some("".into()),
		})
}
