// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0012_discard_json_banks", "knowledge")
		.add_dependency("knowledge", "0011_memory_jobs")
		.add_dependency("execution", "0007_model_state")
		.add_operation(Operation::DropTable {
			name: "semantic_agent_memory".into(),
		})
		.add_operation(Operation::DropTable {
			name: "memory".into(),
		})
		.atomic(true)
}
