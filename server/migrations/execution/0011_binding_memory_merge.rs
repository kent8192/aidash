// reinhardt-migration-source: 1
// Join the Home continuation and native memory state histories without rewriting them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0011_binding_memory_merge", "execution")
		.add_dependency("execution", "0010_native_memory_model_state")
		.add_dependency("execution", "0009_home_waiting_references_state")
		.state_only(true)
}
