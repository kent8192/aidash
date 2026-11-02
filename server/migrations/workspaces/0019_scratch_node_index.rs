// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0019_scratch_node_index", "workspaces")
		.add_dependency("workspaces", "0018_scratch_space_model_state")
		.add_operation(Operation::CreateIndex {
			table: "scratch_spaces".into(),
			columns: vec!["tenant".into(), "node".into()],
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
