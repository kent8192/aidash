// reinhardt-migration-source: 1
// The Workspace is dissolved into Threads (#1008, ADR 0140): the Workspace
// ownership table becomes `authorization_threads` and every `workspace_id` of
// this app becomes `thread_id`. Thread Approval Modes named reply threads,
// which are gone, so their rows are dropped; the deletion is Reinhardt Query
// output from `workspaces::services::thread_backfill`, checked by `manage
// migrationseeds`. Constraint and index renames are SQL: Reinhardt has no
// typed operation for them. Reversal restores the schema, not the dropped
// Approval Modes.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0110_threads", "identity")
		.add_dependency("identity", "0109_agent_created_nodes")
		.add_dependency("workspaces", "0049_threads")
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0110_reply_thread_modes.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0110_reply_thread_modes.sql").into()),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_workspaces".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_execution".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_remote_outputs".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_run_outputs".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "authorization_run_reads".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameTable {
			old_name: "authorization_workspaces".into(),
			new_name: "authorization_threads".into(),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0110_thread_names.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0110_thread_names.sql").into()),
		})
}
