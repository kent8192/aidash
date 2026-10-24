// reinhardt-migration-source: 1
// Source identity remains, with insertion-time same-Tenant KeyShare existence.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0047_task_origin_source_lifetime", "identity")
		.add_dependency("identity", "0046_receiving_keysets_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "authorization_task_origins".into(),
			constraint: Constraint::ForeignKey {
				name: "authorization_task_origins_source_run_id_fkey".into(),
				columns: vec!["tenant".into(), "source_run_id".into()],
				referenced_table: "runs".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "authorization_execution".into(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_task_id_fkey".into(),
				columns: vec!["tenant".into(), "task_id".into()],
				referenced_table: "tasks".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "authorization_execution".into(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_workspace_id_fkey".into(),
				columns: vec!["tenant".into(), "workspace_id".into()],
				referenced_table: "authorization_workspaces".into(),
				referenced_columns: vec!["tenant".into(), "workspace_id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "authorization_run_reads".into(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_reads_workspace_id_fkey".into(),
				columns: vec!["tenant".into(), "workspace_id".into()],
				referenced_table: "authorization_workspaces".into(),
				referenced_columns: vec!["tenant".into(), "workspace_id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
