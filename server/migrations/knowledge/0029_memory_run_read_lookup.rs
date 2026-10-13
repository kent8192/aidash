// reinhardt-migration-source: 1
// A physical lookup index does not change the scalar ORM model state. The
// primary key leads with `run_id`, so impact inspection by Unit needs its own
// key in the order that lookup scans.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0029_memory_run_read_lookup", "knowledge")
		.add_dependency("knowledge", "0028_provider_credentials")
		.add_operation(Operation::CreateIndex {
			table: "memory_run_reads".into(),
			columns: vec!["unit_id".into(), "run_id".into(), "revision".into()],
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
