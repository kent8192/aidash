// reinhardt-migration-source: 1
// PostgreSQL function bodies are DDL and have no typed migration operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0065_native_context_recovery", "registry")
		.add_dependency("registry", "0064_agent_default_removals")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0065_native_context_recovery.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0065_native_context_recovery.sql").into()),
		})
}
