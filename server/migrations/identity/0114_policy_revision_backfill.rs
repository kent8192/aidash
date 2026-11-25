// reinhardt-migration-source: 1
// Repair missing history for current policy bundles on previously upgraded databases.
// Preserve repaired rows on reversal because decisions may already reference them.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0114_policy_revision_backfill", "identity")
		.add_dependency("identity", "0113_budget_notice_delivery")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0114_policy_revision_backfill.sql").to_owned(),
			reverse_sql: Some("SELECT 1;\n".to_owned()),
		})
}
