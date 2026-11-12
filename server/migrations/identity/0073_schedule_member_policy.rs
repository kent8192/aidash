// reinhardt-migration-source: 1
// Upgrade only the unchanged default member policy; custom policies are preserved.
// Rollback leaves grants in place because subsequent policy edits cannot be distinguished.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0073_schedule_member_policy", "identity")
		.add_dependency("identity", "0072_remote_execution_activation")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0073_schedule_member_policy.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0073_schedule_member_policy.sql").to_owned(),
			),
		})
}
