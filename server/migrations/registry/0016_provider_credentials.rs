// reinhardt-migration-source: 1
// PostgreSQL JSONB predicates are not expressible in Reinhardt SchemaExpr yet.
// Preserve the preceding complete constraints, adding only the closed catalog source.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0016_provider_credentials", "registry")
		.add_dependency("registry", "0015_binding_memory_merge")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0016_provider_credentials.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0016_provider_credentials.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
