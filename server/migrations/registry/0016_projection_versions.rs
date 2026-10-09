// reinhardt-migration-source: 1
// Admit Agent projection_version and model projection_versions (ADR 0015).
// PostgreSQL function bodies and JSONB CHECK expressions require DDL SQL assets.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0016_projection_versions", "registry")
		.add_dependency("registry", "0015_binding_memory_merge")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0016_projection_versions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0016_projection_versions.sql").into()),
		})
		.atomic(true)
}
