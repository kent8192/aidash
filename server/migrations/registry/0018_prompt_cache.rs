// reinhardt-migration-source: 1
// Admit Agent prompt_cache and model cache_mode (ADR 0019).
// PostgreSQL function bodies, catalog-driven JSONB CHECK edits and the
// rollback precondition guard require DDL SQL assets.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0018_prompt_cache", "registry")
		.add_dependency("registry", "0017_projection_versions")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0018_prompt_cache.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0018_prompt_cache.sql").into()),
		})
		.atomic(true)
}
