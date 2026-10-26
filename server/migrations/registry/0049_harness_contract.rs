// reinhardt-migration-source: 1
// The Agent Bindings JSON contract accepts the optional Harness Contract
// version (#531). PostgreSQL function bodies have no typed migration
// operation, so the replacement lives in SQL assets. Rollback restores the
// 0045_agent_plugin_settings body.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0049_harness_contract", "registry")
		.add_dependency("registry", "0048_context_marketplace_merge")
		.add_dependency("registry", "0045_agent_plugin_settings")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0049_harness_contract.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0049_harness_contract.sql").into()),
		})
}
