// reinhardt-migration-source: 1
//! Model state for `0028_run_only_tool_answers` (#989).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0029_run_only_tool_answers_state", "federation")
		.add_dependency("federation", "0028_run_only_tool_answers")
		.state_only(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_admissions".into(),
			column: ColumnDefinition::new("run_tool_allowance", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_grants".into(),
			column: ColumnDefinition::new("tool_request_additions", FieldType::Jsonb),
			mysql_options: None,
		})
}
