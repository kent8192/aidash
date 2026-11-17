// reinhardt-migration-source: 1
//! The tools the people of one side allowed "for this Run only" (ADR 0139, #989). A receiving
//! Node keeps them per admission (`run_tool_allowance`) and the Home per Remote Grant
//! (`tool_request_additions`); each is a Federation tool list, and NULL means none. Aidash is
//! undeployed, so no existing row is rewritten.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0028_run_only_tool_answers", "federation")
		.add_dependency("federation", "0027_federation_tool_lists_state")
		.database_only(true)
		.atomic(true)
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
