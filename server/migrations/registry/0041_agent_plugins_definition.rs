// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation, so the Agent
// contract replacement lives in SQL assets. The body keeps every key of
// 0029_completion_policy and adds the #414 `agent_plugins` references (ADR
// 0072). Its reversal reads Agent definitions and the Binding snapshots Runs
// pin, because the plugin copies live in their own table.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0041_agent_plugins_definition", "registry")
		.add_dependency("registry", "0040_agent_plugin_copies_row_level_security")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0041_agent_plugins_definition.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0041_agent_plugins_definition.sql").into(),
			),
		})
}
