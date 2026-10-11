// reinhardt-migration-source: 1
// PostgreSQL trigger function replacement is not expressible through Reinhardt schema operations.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0016_outbound_approval_activation", "execution")
		.database_only(true)
		.add_dependency("execution", "0015_inference_progress_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0016_outbound_approval_activation.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0016_outbound_approval_activation.sql").to_owned(),
			),
		})
}
