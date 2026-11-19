// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// Admit the `core.nodes@1` builtins `node_create` and `node_delete` (#1012),
// which an Agent binds explicitly; they are never default tools.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0080_node_tools", "registry")
		.add_dependency("registry", "0079_workbench_input_bytes")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0080_node_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0080_node_tools.sql").into()),
		})
}
