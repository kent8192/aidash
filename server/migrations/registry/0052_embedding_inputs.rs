// reinhardt-migration-source: 1
// Extend the current PostgreSQL JSONB contract without rewriting existing data.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0052_embedding_inputs", "registry")
		.add_dependency("registry", "0049_plugin_context_merge")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0052_embedding_inputs.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0052_embedding_inputs.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
