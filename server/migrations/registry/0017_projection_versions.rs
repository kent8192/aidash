// reinhardt-migration-source: 1
// Preserve applied definitions, including the Provider Credential allowlist of
// 0016_provider_credentials; widen the Agent and model allowlists with
// Projection Version declarations (ADR 0015).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0017_projection_versions", "registry")
		.add_dependency("registry", "0016_provider_credentials")
		// PostgreSQL function bodies and JSONB CHECK expressions live in SQL
		// assets. The filesystem migration loader supports external assets
		// through RunSQL only.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0017_projection_versions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0017_projection_versions.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
