// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_default_scratch_index", "workspaces")
		.add_dependency("workspaces", "0072_node_scratch_spaces")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateIndex {
			table: "scratch_spaces".into(),
			columns: vec!["tenant".into(), "node".into(), "is_default".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("is_default".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
