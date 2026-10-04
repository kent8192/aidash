// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "workspaces")
		.database_only(true)
		.add_dependency("registry", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_task_workspace".to_owned(),
				columns: vec!["task_id".to_owned(), "workspace_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_workspace_id_message_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned(), "message_id".to_owned()],
				referenced_table: "messages".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_message_context".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "channel_message_context_workspace_id_message_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned(), "message_id".to_owned()],
				referenced_table: "messages".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_message_context".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "channel_message_context_workspace_id_thread_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned(), "thread_id".to_owned()],
				referenced_table: "channel_threads".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_threads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "channel_threads_workspace_id_root_message_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned(), "root_message_id".to_owned()],
				referenced_table: "messages".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "conversations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "conversations_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "messages".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "messages_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_dependencies".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "tasks_dependencies_source_workspace".to_owned(),
				columns: vec!["task_id".to_owned(), "workspace_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_dependencies".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "tasks_dependencies_target_workspace".to_owned(),
				columns: vec!["dependency_id".to_owned(), "workspace_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "tasks_parent_id_fkey".to_owned(),
				columns: vec!["parent_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "tasks_parent_workspace".to_owned(),
				columns: vec!["parent_id".to_owned(), "workspace_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "tasks_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
