// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0051_provider_warning_tools", "registry")
		.add_dependency("registry", "0050_provider_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0051_provider_warning_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0051_provider_warning_tools.sql").into()),
		})
}
