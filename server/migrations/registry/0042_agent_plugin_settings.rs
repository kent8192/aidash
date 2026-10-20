// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation, so the Agent
// contract replacement lives in SQL assets. The body keeps every key of
// 0041_agent_plugins_definition and adds the #414 `agent_plugin_settings` key
// (ADR 0072): what an Agent version decides about its plugins' servers. Its
// reversal reads Agent definitions and the Binding snapshots Runs pin.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0042_agent_plugin_settings", "registry")
		.add_dependency("registry", "0041_agent_plugins_definition")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0042_agent_plugin_settings.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0042_agent_plugin_settings.sql").into()),
		})
}
