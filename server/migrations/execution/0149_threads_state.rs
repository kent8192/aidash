// reinhardt-migration-source: 1
// ORM state of the removed Task to reply-thread map; 0148 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0149_threads_state", "execution")
		.add_dependency("execution", "0148_threads")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "core_task_sessions".into(),
		})
}
