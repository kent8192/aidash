// reinhardt-migration-source: 1
// Stored policies name the Thread actions, resource kind and attribute
// instead of the Workspace ones (#1008, ADR 0140): `workspace.*` becomes
// `thread.*`, the `workspace` kind `thread` and the `workspace_id` attribute
// `thread_id`, in every policy, custom ones included. Rewriting stored
// policies is data, not schema: the assets are Reinhardt Query output from
// `identity::services::thread_policy_backfill`, checked by `manage
// migrationseeds`, and open the Deployment Scope the forced row-level policies
// require.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0111_thread_member_policy", "identity")
		.add_dependency("identity", "0110_threads")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0111_thread_member_policy.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0111_thread_member_policy.sql").to_owned(),
			),
		})
}
