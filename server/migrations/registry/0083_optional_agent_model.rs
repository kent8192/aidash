// reinhardt-migration-source: 1
// PostgreSQL trigger and JSON validator function bodies require DDL.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0083_optional_agent_model", "registry")
		.add_dependency("registry", "0082_thread_source_scope")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_optional_agent_model.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0083_optional_agent_model.sql").into()),
		})
}
