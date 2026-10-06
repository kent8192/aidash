// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "identity")
		.database_only(true)
		.add_dependency("federation", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_catalog".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_catalog_entry_id_entry_version_fkey".to_owned(),
				columns: vec!["entry_id".to_owned(), "entry_version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_catalog_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_catalog_history_tenant_entry_id_entry_versio_fkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
				],
				referenced_table: "authorization_catalog".to_owned(),
				referenced_columns: vec![
					"tenant".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
				],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_catalog".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_catalog_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_credentials".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_credentials_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_decisions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_decisions_tenant_revision_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "revision".to_owned()],
				referenced_table: "authorization_revisions".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "revision".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_credential_id_fkey".to_owned(),
				columns: vec!["credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_execution_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "authorization_workspaces".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_graph_operator_grants".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_graph_operator_grants_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_revisions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_revisions_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_outputs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_outputs_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_reads_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_reads_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "authorization_workspaces".to_owned(),
				referenced_columns: vec!["workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_registry_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_registry_reads_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_remote_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_run_remote_reads_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_task_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_task_origins_source_run_id_fkey".to_owned(),
				columns: vec!["source_run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_task_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_task_origins_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_task_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_task_origins_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_workspaces".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_workspaces_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_workspaces".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_workspaces_workspace_id_fkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_execution_origins_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_execution_origins_mapping_id_fkey".to_owned(),
				columns: vec!["mapping_id".to_owned()],
				referenced_table: "dashboard_mappings".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_execution_origins_run_id_fkey".to_owned(),
				columns: vec!["run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_mappings_credential_id_fkey".to_owned(),
				columns: vec!["credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_mappings_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_operator_grants".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_operator_grants_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_registration_requests_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_sessions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_sessions_identity_id_fkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
				referenced_table: "dashboard_identities".to_owned(),
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
