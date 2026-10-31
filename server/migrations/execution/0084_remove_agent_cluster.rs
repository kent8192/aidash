// reinhardt-migration-source: 1
// PostgreSQL JSONB key removal has no typed Reinhardt migration operation.
// SET LOCAL opens the owner transaction's Deployment Scope before accessing
// the forced row-level policies; the native migration executor has no scope API.
// Policy templates and generated definitions are rewritten without touching
// Run snapshots. The removed references cannot be reconstructed on rollback.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0084_remove_agent_cluster", "execution")
		.add_dependency("execution", "0083_harness_contract_state")
		.add_dependency("registry", "0053_remove_agent_cluster")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0084_remove_agent_cluster.sql").into(),
			reverse_sql: None,
		})
}
