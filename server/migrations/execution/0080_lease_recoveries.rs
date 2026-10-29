// reinhardt-migration-source: 1
// PostgreSQL function replacement and JSON data conversion are not expressible
// with Reinhardt schema operations; the physical default is database-only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0080_lease_recoveries", "execution")
		.add_dependency("execution", "0079_run_safety_stops_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0080_lease_recoveries.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0080_lease_recoveries.sql").to_owned()),
		})
}
