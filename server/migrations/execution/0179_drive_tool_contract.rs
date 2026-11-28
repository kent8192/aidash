// reinhardt-migration-source: 1
//! Admit the Drive tools and the optional drive_search default binding.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0179_drive_tool_contract", "execution")
		.add_dependency("execution", "0178_drive_departed_folders")
		.add_dependency("registry", "0082_thread_source_scope")
		.database_only(true)
		.atomic(true)
		// Reinhardt has no typed operation for PostgreSQL validation function bodies.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0179_drive_tool_contract.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0179_drive_tool_contract.sql").into()),
		})
}
