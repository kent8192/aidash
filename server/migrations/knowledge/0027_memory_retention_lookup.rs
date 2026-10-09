// reinhardt-migration-source: 1
// A physical lookup index does not change the scalar ORM model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0027_memory_retention_lookup", "knowledge")
		.add_dependency("knowledge", "0026_memory_decay_model_state")
		.add_operation(Operation::CreateIndex {
			table: "memory_unit_retention".into(),
			columns: vec!["bank_id".into(), "dormant_policy".into(), "pinned".into()],
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
