// reinhardt-migration-source: 1
// The built-in package indexes core.npm and core.pypi are removed (#745,
// #729): drafts keep no Binding to them. Reinhardt has no data operation for
// rewriting a JSON array inside a column, so this one-time data change is SQL.
// Registered Agent versions are immutable and keep theirs.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0069_remove_package_index_draft_bindings", "registry")
		.add_dependency("registry", "0068_reject_new_compaction_deciders")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0069_remove_package_index_draft_bindings.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0069_remove_package_index_draft_bindings.sql").into(),
			),
		})
}
