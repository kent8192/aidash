// reinhardt-migration-source: 1
// The descriptor contract is a PostgreSQL function body, which has no typed
// Reinhardt operation. Rollback restores the 0019 body (0020 leaves it
// unchanged). It does not delete research descriptors already registered; the
// NOT VALID registry checks then reject their later rewrites, so remove
// research packages before rolling back.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0021_research_providers", "registry")
		.add_dependency("registry", "0020_model_streaming_config")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0021_research_providers.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0021_research_providers.sql").into()),
		})
}
