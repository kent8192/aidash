// reinhardt-migration-source: 1
// A physical queue lookup index does not alter ORM model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0045_image_publication_lookup", "knowledge")
		.add_dependency("knowledge", "0044_image_ledger_workspace")
		.add_operation(Operation::CreateIndex {
			table: "semantic_image_publications".into(),
			columns: vec!["workspace_id".into(), "sequence".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.atomic(true)
		.database_only(true)
}
