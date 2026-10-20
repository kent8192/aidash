// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_foreign_key_relations_state", "execution")
		.add_dependency("execution", "0045_decision_routes_state")
		.add_operation(Operation::AddConstraintDefinition {
			table: "context_compaction_attempts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "context_compaction_attempt_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_string(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_area_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "area_id".to_string()],
				referenced_table: "core_areas".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_attempts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "decision_attempt_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_evidence".to_string(),
			constraint: Constraint::ForeignKey {
				name: "decision_evidence_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_outcomes".to_string(),
			constraint: Constraint::ForeignKey {
				name: "decision_outcome_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_string(),
			constraint: Constraint::ForeignKey {
				name: "events_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_compaction_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_compaction_usage_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_history".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_history_request_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "request_id".to_string()],
				referenced_table: "generation_requests".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_intents_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_remote_intents_task_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "task_id".to_string()],
				referenced_table: "tasks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_requests_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_summary_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_summary_usage_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_usage_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_string(),
			constraint: Constraint::ForeignKey {
				name: "human_requests_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_attempts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "inference_attempt_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_progress".to_string(),
			constraint: Constraint::ForeignKey {
				name: "inference_progress_attempt".to_string(),
				columns: vec!["tenant".to_string(), "attempt_id".to_string()],
				referenced_table: "inference_attempts".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "inference_usage_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".to_string(),
			constraint: Constraint::ForeignKey {
				name: "invocations_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "research_attempts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "research_attempts_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "research_sources".to_string(),
			constraint: Constraint::ForeignKey {
				name: "research_sources_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_inputs".to_string(),
			constraint: Constraint::ForeignKey {
				name: "run_inputs_run_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_task_snapshots".to_string(),
			constraint: Constraint::ForeignKey {
				name: "run_task_snapshot_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "url_intents".to_string(),
			constraint: Constraint::ForeignKey {
				name: "url_intents_run".to_string(),
				columns: vec!["tenant".to_string(), "run_id".to_string()],
				referenced_table: "runs".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
