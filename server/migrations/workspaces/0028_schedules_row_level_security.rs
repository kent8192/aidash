// reinhardt-migration-source: 1
// Reinhardt does not express row policies; use the tenant policy template.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0028_schedules_row_level_security", "workspaces")
		.add_dependency("workspaces", "0027_schedules")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0028_schedules_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0028_schedules_row_level_security.sql").to_owned(),
			),
		})
}
