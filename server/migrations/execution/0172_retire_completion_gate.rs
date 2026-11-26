// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0172_retire_completion_gate", "execution")
		.add_dependency("execution", "0171_outside_versions")
		.add_dependency("registry", "0082_thread_source_scope")
		.add_dependency("workspaces", "0071_completion_messages")
		.add_dependency("identity", "0123_goal_judge_model")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			// Data statements are Reinhardt Query output. The scope and
			// PostgreSQL trigger lifecycle have no typed migration operation.
			sql: include_str!("sql/forward/0172_goal_backfill.sql").into(),
			reverse_sql: Some("SELECT 1;".into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0172_goal_registry_contract.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0172_goal_registry_contract.sql").into()),
		})
}
