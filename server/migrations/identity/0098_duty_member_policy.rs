// reinhardt-migration-source: 1
// Stored policies name the Duty actions instead of the Schedule actions
// (#1011), and the unchanged default member policy gains `subnode.create`
// (#1012); custom policies keep what their Tenant chose. Rewriting stored
// policies is data, not schema: the assets are Reinhardt Query output from
// `identity::services::duty_actions_backfill`, checked by `manage
// migrationseeds`, and open the Deployment Scope the forced row-level policies
// require.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0098_duty_member_policy", "identity")
		.add_dependency("identity", "0097_unscheduled_executions")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0098_duty_member_policy.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0098_duty_member_policy.sql").to_owned()),
		})
}
