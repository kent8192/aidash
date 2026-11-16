// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed Reinhardt operation; SQL is DDL only.
// Existing compaction Decider rows remain readable; application admission rejects new ones.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0065_agent_default_removals", "registry")
		.add_dependency("registry", "0064_journal_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0065_agent_default_removals.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0065_agent_default_removals.sql").into()),
		})
}
