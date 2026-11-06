// reinhardt-migration-source: 1
// A paused refusal of a replay-safe tool ends REFUSED (#754). The historical CHECK is
// database-only; 0121 tracks the typed ModelEnum separately.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0120_invocation_refused", "execution")
		.add_dependency("execution", "0118_blob_uploads")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "invocations".into(),
			constraint: Constraint::Check {
				name: "invocations_status_check".into(),
				expression: "(status = ANY (ARRAY['STARTED'::text, 'COMPLETED'::text, 'UNCERTAIN'::text]))".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".into(),
			constraint: Constraint::Check {
				name: "invocations_status_check".into(),
				expression: "(status = ANY (ARRAY['STARTED'::text, 'COMPLETED'::text, 'UNCERTAIN'::text, 'REFUSED'::text]))".into(),
			},
		})
}
