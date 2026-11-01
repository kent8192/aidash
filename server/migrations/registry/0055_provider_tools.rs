// reinhardt-migration-source: 1
// PostgreSQL validation function bodies have no typed migration operation.
// DDL only: extend the installed contracts without rewriting immutable entries.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0055_provider_tools", "registry")
		.add_dependency("registry", "0054_remove_agent_cluster")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0055_provider_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0055_provider_tools.sql").into()),
		})
}
