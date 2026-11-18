// reinhardt-migration-source: 1
// The Agent Bindings JSON contract accepts the `structured@4` Harness Contract
// version (#1010), which states Instruction precedence. PostgreSQL function
// bodies have no typed migration operation, so the replacement lives in SQL
// assets. Rollback restores the 0074_remove_research_providers body.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0075_harness_contract_structured4", "registry")
		.add_dependency("registry", "0074_remove_research_providers")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0075_harness_contract_structured4.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0075_harness_contract_structured4.sql").into(),
			),
		})
}
