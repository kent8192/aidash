// reinhardt-migration-source: 1
// The Agent Bindings JSON contract stops accepting the removed `structured@1`
// and `structured@2` Harness Contract versions (#532). PostgreSQL function
// bodies have no typed migration operation, so the replacement lives in SQL
// assets. The forward step refuses while stored definitions, packages or Run
// Binding snapshots still declare a removed version. Rollback restores the
// 0052_harness_contract_structured3 body.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0054_remove_structured_harness_contracts", "registry")
		.add_dependency("registry", "0053_connection_request")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0054_remove_structured_harness_contracts.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0054_remove_structured_harness_contracts.sql").into(),
			),
		})
}
