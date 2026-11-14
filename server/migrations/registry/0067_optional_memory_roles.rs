// reinhardt-migration-source: 1
// PostgreSQL trigger function bodies have no typed migration operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0067_optional_memory_roles", "registry")
		.add_dependency("registry", "0066_native_context_recovery")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0067_optional_memory_roles.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0067_optional_memory_roles.sql").into()),
		})
}
