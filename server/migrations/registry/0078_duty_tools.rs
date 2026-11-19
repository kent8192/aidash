// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// The Schedule tools become the Duty tools of `core.duties@1` (#1011).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0078_duty_tools", "registry")
		.add_dependency("registry", "0077_tool_parallelism_64")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0078_duty_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0078_duty_tools.sql").into()),
		})
}
