// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0124_remove_compaction_decision_storage_state", "execution")
		.add_dependency("execution", "0123_remove_compaction_decision_storage")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "decision_states".into(),
		})
		.add_operation(Operation::DropTable {
			name: "decision_outcomes".into(),
		})
}
