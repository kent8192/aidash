// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0069_schedule_tools", "registry")
		.add_dependency("registry", "0068_reject_new_compaction_deciders")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0069_schedule_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0069_schedule_tools.sql").into()),
		})
}
