// reinhardt-migration-source: 1
// PostgreSQL function bodies cannot be represented by typed migration operations.
// Extend the built-in descriptor contract and restore its previous body on rollback.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0053_subnode_provider", "registry")
		.add_dependency("registry", "0052_harness_contract_structured3")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0053_subnode_provider.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0053_subnode_provider.sql").into()),
		})
}
