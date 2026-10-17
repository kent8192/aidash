// reinhardt-migration-source: 1
// The functions every Tenant-owned table's row-level policy calls (#264,
// ADR 0040). They read the tenancy scope settings; each app's
// row_level_security migration enforces the policy on its own tables.
// Reinhardt has no operation for functions with a pinned search_path, so the
// definitions are SQL assets (kent8192/reinhardt-web#6507).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0026_tenancy_policy_functions", "identity")
		.add_dependency("identity", "0025_tenant_retention_rules_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0026_tenancy_policy_functions.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0026_tenancy_policy_functions.sql").to_owned(),
			),
		})
}
