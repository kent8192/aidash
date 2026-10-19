// reinhardt-migration-source: 1
// Enforce the forced row-level policy on the tables that gained a `tenant`
// column in #265 (ADR 0040). Reinhardt has no policy operations
// (kent8192/reinhardt-web#6770), so the statements are SQL assets rendered by
// `server/src/database/policy.rs`; a test compares them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0043_tenant_owned_row_level_security", "execution")
		.add_dependency("execution", "0042_decision_evidence_v3_state")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0043_tenant_owned_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0043_tenant_owned_row_level_security.sql").to_owned(),
			),
		})
}
