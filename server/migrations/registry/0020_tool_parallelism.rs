// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation and the
// catalog-driven installation guard allowlist rewrite requires DDL SQL assets.
// The Agent contract keeps the 0017 Projection Version, 0018 prompt_cache and
// 0019 deferred exposure keys.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0020_tool_parallelism", "registry")
		.add_dependency("registry", "0019_deferred_exposure")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0020_tool_parallelism.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0020_tool_parallelism.sql").into()),
		})
}
