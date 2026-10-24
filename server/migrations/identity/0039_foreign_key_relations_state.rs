// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0039_foreign_key_relations_state", "identity")
		.add_dependency("identity", "0038_tenant_secrets_state")
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_task_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "task_id".to_string()],
				referenced_table: "tasks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "authorization_workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_reads".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_reads_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "authorization_workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_task_origins".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_task_origins_source_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "source_run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_execution_origins_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_execution_origins_mapping_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "mapping_id".to_string()],
				referenced_table: "dashboard_mappings".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_mappings_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_mappings_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_registration_requests_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_registration_requests_invitation_id_fkey".to_string(),
				columns: vec!["invitation_id".to_string()],
				referenced_table: "tenant_invitations".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_sessions".to_string(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_sessions_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "desktop_handoffs".to_string(),
			constraint: Constraint::ForeignKey {
				name: "desktop_handoffs_browser_session_id_fkey".to_string(),
				columns: vec!["browser_session_id".to_string()],
				referenced_table: "dashboard_sessions".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "desktop_refresh_credentials".to_string(),
			constraint: Constraint::ForeignKey {
				name: "desktop_refresh_credentials_session_id_fkey".to_string(),
				columns: vec!["session_id".to_string()],
				referenced_table: "dashboard_sessions".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "plan_connections".to_string(),
			constraint: Constraint::ForeignKey {
				name: "plan_connections_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_plan_connections".to_string(),
			constraint: Constraint::ForeignKey {
				name: "run_plan_connections_identity_id_fkey".to_string(),
				columns: vec!["identity_id".to_string()],
				referenced_table: "dashboard_identities".to_string(),
				referenced_columns: vec!["id".to_string()],
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
