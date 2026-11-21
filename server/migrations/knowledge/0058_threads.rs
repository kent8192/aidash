// reinhardt-migration-source: 1
// The Workspace is dissolved into Threads (#1008, ADR 0140): the semantic
// index and the memory banks are per Thread, so every `workspace_id` of this
// app becomes `thread_id`. Constraint and index renames and the guard trigger
// bodies are SQL: Reinhardt has no typed operation for them.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0058_threads", "knowledge")
		.add_dependency("knowledge", "0057_journal_proofs_without_research_state")
		.add_dependency("workspaces", "0049_threads")
		.atomic(true)
		.add_operation(Operation::RenameColumn {
			table: "memory_banks".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "memory_participants".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_collections".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_entries".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_history".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_image_ledgers".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_image_publications".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_indexes".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "semantic_vectors".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0058_thread_names.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0058_thread_names.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0058_thread_functions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0058_thread_functions.sql").into()),
		})
}
