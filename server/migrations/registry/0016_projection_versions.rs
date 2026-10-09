// reinhardt-migration-source: 1
// Preserve applied definitions; widen the Agent and model allowlists with
// Projection Version declarations (ADR 0015).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0016_projection_versions", "registry")
		.add_dependency("registry", "0015_binding_memory_merge")
		// PostgreSQL function bodies and JSONB CHECK expressions live in SQL
		// assets. The filesystem migration loader supports external assets
		// through RunSQL only.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0016_projection_versions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0016_projection_versions.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
