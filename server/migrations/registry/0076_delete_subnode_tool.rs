// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// The Subnode tool `subnode_dissolve` is renamed `delete_subnode` (#1012).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0076_delete_subnode_tool", "registry")
		.add_dependency("registry", "0075_harness_contract_structured4")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0076_delete_subnode_tool.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0076_delete_subnode_tool.sql").into()),
		})
}
