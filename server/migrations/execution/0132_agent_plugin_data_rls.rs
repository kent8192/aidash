// reinhardt-migration-source: 1
// Reinhardt migrations do not express row-level policies.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0132_agent_plugin_data_rls", "execution")
		.add_dependency("execution", "0131_agent_plugin_data")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0132_agent_plugin_data_rls.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0132_agent_plugin_data_rls.sql").into()),
		})
}
