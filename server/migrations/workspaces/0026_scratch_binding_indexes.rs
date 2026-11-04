// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0026_scratch_binding_indexes", "workspaces")
		.add_dependency("workspaces", "0025_scratch_workspace_model_state")
		.add_operation(Operation::CreateIndex {
			table: "workspaces".into(),
			columns: vec!["tenant".into(), "scratch_space_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "conversations".into(),
			columns: vec!["tenant".into(), "scratch_space_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.database_only(true)
		.atomic(true)
}
