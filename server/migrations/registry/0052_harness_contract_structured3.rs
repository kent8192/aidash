// reinhardt-migration-source: 1
// The Agent Bindings JSON contract accepts the `structured@3` Harness Contract
// version (#532). PostgreSQL function bodies have no typed migration
// operation, so the replacement lives in SQL assets. Rollback restores the
// 0050_harness_contract body.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0052_harness_contract_structured3", "registry")
		.add_dependency("registry", "0051_harness_plugin_merge")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0052_harness_contract_structured3.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0052_harness_contract_structured3.sql").into(),
			),
		})
}
