// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
// A Human Request goes with its Run: a Run points back at the Request it waits
// for, so only the cascade lets a purge delete both (ADR 0061).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "execution")
		.database_only(true)
		.add_dependency("workspaces", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_area_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "area_id".to_owned()],
				referenced_table: "core_areas".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "events_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_budgets".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_budgets_request_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "request_id".to_owned()],
				referenced_table: "generation_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_compaction_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_compaction_usage_request_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "request_id".to_owned()],
				referenced_table: "generation_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_compaction_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_compaction_usage_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_request_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "request_id".to_owned()],
				referenced_table: "generation_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_history_request_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "request_id".to_owned()],
				referenced_table: "generation_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policies".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_policies_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policy_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_policy_history_tenant_policy_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "policy_id".to_owned()],
				referenced_table: "generation_policies".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_intents_credential_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_intents_task_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_credential_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_local_task_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "local_task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_local_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "local_workspace_id".to_owned()],
				referenced_table: "authorization_workspaces".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_task_workspace".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"local_task_id".to_owned(),
					"local_workspace_id".to_owned(),
				],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec![
					"tenant".to_owned(),
					"id".to_owned(),
					"workspace_id".to_owned(),
				],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_tenant_policy_id_policy_revision_fkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"policy_id".to_owned(),
					"policy_revision".to_owned(),
				],
				referenced_table: "generation_policy_history".to_owned(),
				referenced_columns: vec![
					"tenant".to_owned(),
					"policy_id".to_owned(),
					"revision".to_owned(),
				],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_usage_request_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "request_id".to_owned()],
				referenced_table: "generation_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_usage_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "human_requests_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "human_requests_run_workspace".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"run_id".to_owned(),
					"workspace_id".to_owned(),
				],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec![
					"tenant".to_owned(),
					"id".to_owned(),
					"workspace_id".to_owned(),
				],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "invocations_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_inputs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "run_inputs_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "runs_human_request_ref".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"pending_human_request_id".to_owned(),
					"id".to_owned(),
				],
				referenced_table: "human_requests".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned(), "run_id".to_owned()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_areas".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_areas_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_objects".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_objects_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_operations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_operations_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_quotas".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_quotas_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_records".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_records_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_requests_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_task_sessions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "core_task_sessions_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "events_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "events_node_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "node".to_owned()],
				referenced_table: "nodes".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "runs_node_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "node".to_owned()],
				referenced_table: "nodes".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_node_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "node".to_owned()],
				referenced_table: "nodes".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_budgets".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_budgets_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_compaction_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_compaction_usage_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_history_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policies".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_policies_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policy_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_policy_history_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_dispatches".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_dispatches_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_finalizations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_finalizations_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_intents_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_usage_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_usage".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "generation_usage_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "human_requests_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "invocations_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_activations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "run_activations_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_inputs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "run_inputs_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "runs_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
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
