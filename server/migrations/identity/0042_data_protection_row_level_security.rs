// reinhardt-migration-source: 1
// Enforce the forced row-level policy of Tenant-owned tables (#264, ADR 0040)
// on Data Protection policies and history. Reinhardt has no policy operations
// (kent8192/reinhardt-web#6770), so the statements are SQL assets rendered by
// `server/src/database/policy.rs`; a test compares them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0042_data_protection_row_level_security", "identity")
		.add_dependency("identity", "0041_tenant_data_protection")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0042_data_protection_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0042_data_protection_row_level_security.sql").to_owned(),
			),
		})
}
