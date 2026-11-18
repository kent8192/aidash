// reinhardt-migration-source: 1
// An Agent definition's Tool Parallelism may reach 64 (#1009). PostgreSQL
// function bodies have no typed migration operation, so the replacement lives
// in SQL assets. Rollback restores the 0070 body.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0072_tool_parallelism_64", "registry")
		.add_dependency("registry", "0071_delete_subnode_tool")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0072_tool_parallelism_64.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0072_tool_parallelism_64.sql").into()),
		})
}
