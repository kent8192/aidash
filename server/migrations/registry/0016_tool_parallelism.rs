// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation. DDL only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0016_tool_parallelism", "registry")
		.add_dependency("registry", "0015_binding_memory_merge")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0016_tool_parallelism.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0016_tool_parallelism.sql").into()),
		})
}
