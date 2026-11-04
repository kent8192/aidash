// reinhardt-migration-source: 1
// Upgrade only the unchanged default member policy; custom policies are preserved.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0067_scratch_member_policy", "identity")
		.add_dependency("identity", "0066_node_auth_history_merge")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0067_scratch_member_policy.sql").to_owned(),
			reverse_sql: None,
		})
}
