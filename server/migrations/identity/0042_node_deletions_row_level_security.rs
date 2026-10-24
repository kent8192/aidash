// reinhardt-migration-source: 1
// Reinhardt has no RLS policy operations (#6770). The assets use the existing
// policy renderer, checked against every classified Tenant-owned table.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0042_node_deletions_row_level_security", "identity")
		.add_dependency("identity", "0041_node_deletions_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0042_node_deletions_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0042_node_deletions_row_level_security.sql").to_owned(),
			),
		})
}
