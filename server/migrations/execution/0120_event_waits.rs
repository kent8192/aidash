// reinhardt-migration-source: 1
// PostgreSQL trigger functions are not expressible as Reinhardt operations.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0120_event_waits", "execution")
		.add_dependency("execution", "0119_egress_request_times")
		.add_dependency("identity", "0004_references")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0120_event_waits.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0120_event_waits.sql").to_owned()),
		})
}
