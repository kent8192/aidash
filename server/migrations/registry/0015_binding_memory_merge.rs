// reinhardt-migration-source: 1
// PostgreSQL function bodies and NOT VALID JSON checks require DDL SQL assets.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0015_binding_memory_merge", "registry")
		.add_dependency("registry", "0014_openrouter_embeddings")
		.add_dependency("registry", "0012_host_lifecycle_expression")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0015_binding_memory_merge.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0015_binding_memory_merge.sql").into()),
		})
}
