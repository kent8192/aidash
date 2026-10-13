// reinhardt-migration-source: 1
// The descriptor contract is a PostgreSQL function body, which has no typed
// Reinhardt operation. It extends the 0021_tool_parallelism body, keeping its
// concurrency narrowing rules; rollback restores that body. It does not delete research descriptors already registered; the
// NOT VALID registry checks then reject their later rewrites, so remove
// research packages before rolling back.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0023_research_providers", "registry")
		.add_dependency("registry", "0022_context_policy")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0023_research_providers.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0023_research_providers.sql").into()),
		})
}
