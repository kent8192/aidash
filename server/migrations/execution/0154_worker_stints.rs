// reinhardt-migration-source: 1
// A worker opens a Stint after it takes its turn in the Working Area.
// PostgreSQL trigger function bodies require RunSQL.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0154_worker_stints", "execution")
		.add_dependency("execution", "0153_area_queue_sleep")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0154_worker_stints.sql").into(),
			reverse_sql: Some(include_str!("sql/forward/0152_stint_leases.sql").into()),
		})
}
