// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// Admit the `core.threads@1` builtins `thread_read`, `thread_observe`,
// `thread_search`, `thread_message` and `task_wait` (#1008, ADR 0140), the
// default tools that replace the `core.workspace@1` tools, and let an Agent
// remove them from its defaults. The Workspace tools stay valid: saved Agent
// versions pin them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0081_thread_tools", "registry")
		.add_dependency("registry", "0080_node_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0081_thread_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0081_thread_tools.sql").into()),
		})
}
