// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation, so the Agent
// contract replacement lives in SQL assets. The body keeps the 0017 Projection
// Version, 0018 prompt_cache, 0019 deferred exposure and 0021
// tool_parallelism keys; the installation guard is untouched.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0022_context_policy", "registry")
		.add_dependency("registry", "0021_tool_parallelism")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0022_context_policy.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0022_context_policy.sql").into()),
		})
}
