// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0008_native_memory_usage", "execution")
		.add_dependency("execution", "0007_model_state")
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression: "purpose IN ('embedding', 'inference', 'compaction')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_usage".into(),
			constraint: Constraint::Check {
				name: "generation_remote_usage_purpose_check".into(),
				expression: "purpose IN ('embedding', 'inference', 'compaction', 'memory')".into(),
			},
		})
		.atomic(true)
}
