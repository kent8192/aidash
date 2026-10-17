// reinhardt-migration-source: 1
// Degraded context (#234, ADR 0059): let a `workspace_retrieval` source declare
// `on_unavailable: pause | degrade` by replacing aidash_context_is_valid.
// PostgreSQL function bodies have no typed Reinhardt operation; the filesystem
// migration loader supports external assets through RunSQL only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0031_degraded_context", "registry")
		.database_only(true)
		.add_dependency("registry", "0030_plan_connection_models")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0031_degraded_context.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0031_degraded_context.sql").into()),
		})
		.atomic(true)
}
