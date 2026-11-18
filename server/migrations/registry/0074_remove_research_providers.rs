// reinhardt-migration-source: 1
// Research is no longer a category (#987, ADR 0138). PostgreSQL function
// bodies and rewriting a JSON array inside a column have no typed Reinhardt
// operation, so the descriptor contract and the draft Bindings change in SQL.
// Removed definitions keep their rows so pinned Agent versions resolve (#952).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0074_remove_research_providers", "registry")
		.add_dependency("registry", "0073_agent_plugin_server_secrets")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0074_remove_research_providers.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0074_remove_research_providers.sql").into(),
			),
		})
}
