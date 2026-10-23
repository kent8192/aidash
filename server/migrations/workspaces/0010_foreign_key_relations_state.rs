// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0010_foreign_key_relations_state", "workspaces")
		.add_dependency("workspaces", "0009_tenant_owned_row_level_security")
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_task_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "task_id".to_string()],
				referenced_table: "tasks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_string(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "conversations".to_string(),
			constraint: Constraint::ForeignKey {
				name: "conversations_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "messages".to_string(),
			constraint: Constraint::ForeignKey {
				name: "messages_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "tasks_parent_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "parent_id".to_string()],
				referenced_table: "tasks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "tasks_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
