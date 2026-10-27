// reinhardt-migration-source: 1
// Keeps a Run's control provenance true for every writer of `control` (#485,
// ADR 0101). Reinhardt has no trigger operation, so the function and trigger
// are the documented raw-SQL exception. A writer that knows about provenance
// bumps `control_revision` itself and is left alone; any other writer that
// changes `control` bumps the revision here and is attributed to the system.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0062_run_control_trigger", "execution")
		.add_dependency("execution", "0061_run_control_provenance")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0062_run_control_trigger.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0062_run_control_trigger.sql").to_owned()),
		})
}
