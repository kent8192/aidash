// reinhardt-migration-source: 1
// Reinhardt has no row-policy operation; use the repository tenant-policy template.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0078_run_safety_stops_row_level_security", "execution")
		.add_dependency("execution", "0077_run_safety_stops")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0078_run_safety_stops_row_level_security.sql")
				.to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0078_run_safety_stops_row_level_security.sql")
					.to_owned(),
			),
		})
}
