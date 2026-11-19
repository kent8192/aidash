// reinhardt-migration-source: 1
// ORM state of the removed reply-thread table; 0049 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_threads_state", "workspaces")
		.add_dependency("workspaces", "0049_threads")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "channel_threads".into(),
		})
}
