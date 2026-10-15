// reinhardt-migration-source: 1
// Issue #243 removes the legacy Marketplace, so the scoped Marketplace no longer
// needs a compatibility gate. Its statement trigger has no typed operation, so a
// SQL asset drops it; reversal reinstalls it after the table is recreated.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0008_remove_compatibility_gate", "marketplace")
		.database_only(true)
		.add_dependency("marketplace", "0007_model_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0008_remove_compatibility_gate.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0008_remove_compatibility_gate.sql").into(),
			),
		})
		.add_operation(Operation::DropTable {
			name: "marketplace_gate".into(),
		})
		.atomic(true)
}
