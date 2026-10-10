// reinhardt-migration-source: 1
// Admit Agent projection_version and model projection_versions (ADR 0015).
// PostgreSQL function bodies, catalog-driven JSONB CHECK edits and the
// rollback precondition guard require DDL SQL assets.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_projection_versions", "registry")
		.add_dependency("registry", "0016_provider_credentials")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0017_projection_versions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0017_projection_versions.sql").into()),
		})
		.atomic(true)
}
