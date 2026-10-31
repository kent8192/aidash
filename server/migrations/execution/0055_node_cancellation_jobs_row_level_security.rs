// reinhardt-migration-source: 1
// Reinhardt has no RLS policy operation. This DDL uses the existing Tenant scope policy.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
		"0055_node_cancellation_jobs_row_level_security",
		"execution",
	)
	.add_dependency("execution", "0054_node_cancellation_jobs_state")
	.database_only(true)
	.atomic(true)
	.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/0055_node_cancellation_jobs_row_level_security.sql").into(),
		reverse_sql: Some(
			include_str!("sql/backward/0055_node_cancellation_jobs_row_level_security.sql").into(),
		),
	})
}
