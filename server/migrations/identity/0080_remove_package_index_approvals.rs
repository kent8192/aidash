// reinhardt-migration-source: 1
// The built-in package indexes core.npm and core.pypi are removed (#745,
// #729), so no Tenant keeps a Catalog Approval for them. A one-time data
// deletion that Reinhardt cannot express as a typed operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0080_remove_package_index_approvals", "identity")
		.add_dependency("identity", "0079_bootstrap_reservations_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0080_remove_package_index_approvals.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0080_remove_package_index_approvals.sql").into(),
			),
		})
}
