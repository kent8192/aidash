// reinhardt-migration-source: 1
// The built-in package indexes core.npm and core.pypi are removed (#745,
// #729), so no Tenant keeps a Catalog Approval for them. The SQL assets are
// rendered by the Reinhardt Query builder in
// identity::services::package_index_approvals and checked by
// `manage migrationseeds --check`; reversal does not restore the approvals.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0082_remove_package_index_approvals", "identity")
		.add_dependency("identity", "0081_scheduled_executions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0082_remove_package_index_approvals.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0082_remove_package_index_approvals.sql").into(),
			),
		})
}
