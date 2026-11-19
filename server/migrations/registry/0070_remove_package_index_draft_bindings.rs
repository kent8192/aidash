// reinhardt-migration-source: 1
// The built-in package indexes core.npm and core.pypi are removed (#745,
// #729), and so are the Bindings pointing to them: in Agent drafts, in
// registered Agent versions (their Bindings and the normalized copy saved at
// registration, Marketplace copies included, as registry/0054 did) and in
// registration requests. Reinhardt Query cannot express removing elements from
// a JSON array inside a column, so this one-time data change is SQL.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0070_remove_package_index_draft_bindings", "registry")
		.add_dependency("registry", "0069_schedule_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0070_remove_package_index_draft_bindings.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0070_remove_package_index_draft_bindings.sql").into(),
			),
		})
}
