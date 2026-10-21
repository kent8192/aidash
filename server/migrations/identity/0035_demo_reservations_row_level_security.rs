// reinhardt-migration-source: 1
// Enforce the forced row-level policy on the Demo Reservations (ADR 0040, ADR
// 0079). Reinhardt has no policy operations (kent8192/reinhardt-web#6770), so
// the statements are SQL assets rendered by `server/src/database/policy.rs`; a
// test compares them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0035_demo_reservations_row_level_security", "identity")
		.add_dependency("identity", "0034_demo_data_state")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0035_demo_reservations_row_level_security.sql")
				.to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0035_demo_reservations_row_level_security.sql")
					.to_owned(),
			),
		})
}
