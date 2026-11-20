// reinhardt-migration-source: 1
// The Workspace is dissolved into Threads (#1008, ADR 0140): every
// `workspace_id` of this app becomes `thread_id`, and the Working Area key
// becomes (home Node, Thread, Agent, owner). Areas that differed only by their
// reply thread or Root Task are merged into one per key, and the Task to
// reply-thread map goes. The survivor is the first active or running Area,
// else the first retained or recoverable one, else any, the newest first;
// Runs, operations, objects, records, previews, message file versions and
// image publications that named a merged Area name the survivor, and the
// merged Areas' manifests, holds and Live previews cascade, so files kept in
// a merged Area are dropped. The merge is Reinhardt Query output from
// `workspaces::services::thread_backfill`, checked by
// `manage migrationseeds`. Constraint and index renames, trigger function
// bodies and the forced-policy table drop are SQL: Reinhardt has no typed
// operation for them. Reversal restores the schema only: merged Areas stay
// merged, each gets a fresh session id, and the Task map comes back empty.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0148_threads", "execution")
		.add_dependency("execution", "0147_unscheduled_generations")
		.add_dependency("workspaces", "0049_threads")
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0148_duplicate_areas.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0148_duplicate_areas.sql").into()),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_areas".into(),
			constraint: Constraint::Unique {
				name: "core_session_identity".into(),
				columns: vec![
					"tenant".into(),
					"home_node".into(),
					"workspace_id".into(),
					"thread_id".into(),
					"agent_id".into(),
					"owner".into(),
				],
			},
		})
		// Backward only: drop the default below that restores the column.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0148_session_column.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0148_session_column.sql").into()),
		})
		.add_operation(Operation::DropColumn {
			table: "core_areas".into(),
			column: "thread_id".into(),
			old_definition: Some(
				ColumnDefinition::new("thread_id", FieldType::Uuid)
					.with_not_null(true)
					.with_default(Some("gen_random_uuid()".into())),
			),
		})
		.add_operation(Operation::RenameColumn {
			table: "core_areas".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "runs".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "events".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "human_requests".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "approval_observations".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "generation_embedding_usage".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "generation_requests".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "subnodes".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "generation_requests".into(),
			old_name: "local_workspace_id".into(),
			new_name: "local_thread_id".into(),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_areas".into(),
			constraint: Constraint::Unique {
				name: "core_session_identity".into(),
				columns: vec![
					"tenant".into(),
					"home_node".into(),
					"thread_id".into(),
					"agent_id".into(),
					"owner".into(),
				],
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0148_drop_task_sessions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0148_drop_task_sessions.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0148_thread_names.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0148_thread_names.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0148_thread_functions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0148_thread_functions.sql").into()),
		})
}
