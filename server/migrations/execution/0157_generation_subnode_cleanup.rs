// reinhardt-migration-source: 1
//! Keep pending Subnode cleanup on the retired generation request.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0157_generation_subnode_cleanup", "execution")
		.add_dependency("execution", "0156_model_call_state")
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "generation_requests".into(),
			column: ColumnDefinition::new("subnode_cleanup_at", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "generation_requests".into(),
			columns: vec!["subnode_cleanup_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(subnode_cleanup_at IS NOT NULL)".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
