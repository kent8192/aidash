// reinhardt-migration-source: 1
// PostgreSQL NOT VALID constraint DDL has no typed migration operation.
// CONTRACT(#1019): The integration workspaces migration must update the
// logical ConversationTargetKind enum constraint to the Agent/Node domain.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0091_thread_target_kinds", "registry")
		.add_dependency("registry", "0090_prune_definition_fields")
		.add_dependency("workspaces", "0067_task_duty_start")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0091_thread_target_kinds.sql").into(),
			reverse_sql: None,
		})
}
