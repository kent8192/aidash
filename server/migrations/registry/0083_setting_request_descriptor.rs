// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0083_setting_request_descriptor", "registry")
		.add_dependency("registry", "0082_thread_source_scope")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_setting_request_descriptor.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0083_setting_request_descriptor.sql").into(),
			),
		})
}
