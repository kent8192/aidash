// reinhardt-migration-source: 1
// The Workspace is dissolved into Threads (#1008, ADR 0140): a Remote Grant
// and its reads name the Thread of the Task they were issued for.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0030_threads", "federation")
		.add_dependency("federation", "0029_run_only_tool_answers_state")
		.add_dependency("workspaces", "0049_threads")
		.atomic(true)
		.add_operation(Operation::RenameColumn {
			table: "authorization_remote_grants".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_remote_grant_reads".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
}
