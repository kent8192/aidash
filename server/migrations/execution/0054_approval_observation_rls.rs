// reinhardt-migration-source: 1
// Reinhardt has no row-policy operation; use the repository tenant-policy template.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0054_approval_observation_rls", "execution")
		.add_dependency("execution", "0053_approval_observations")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0054_approval_observation_rls.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0054_approval_observation_rls.sql").to_owned(),
			),
		})
}
