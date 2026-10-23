// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0035_foreign_key_relations_state", "knowledge")
		.add_dependency("knowledge", "0034_tenant_owned_row_level_security")
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_bank_policy_changes".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_policy_change_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_banks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_bank_participant".to_string(),
				columns: vec!["tenant".to_string(), "participant_id".to_string()],
				referenced_table: "memory_participants".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_banks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_bank_workspace".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_candidate_reviews".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_review_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_candidates".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_candidate_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_dependencies".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_dependency_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_engine_jobs".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_job_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_history".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_history_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_model_attempts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_attempt_operation".to_string(),
				columns: vec!["tenant".to_string(), "operation_id".to_string()],
				referenced_table: "memory_model_operations".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_model_operations".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_operation_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_participants".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_participant_workspace".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "workspaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_publications".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_publication_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_publications".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_publication_source".to_string(),
				columns: vec!["tenant".to_string(), "source_id".to_string()],
				referenced_table: "memory_units".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_purge_jobs".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_purge_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_receipts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_receipt_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_run_bindings".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_binding_participant".to_string(),
				columns: vec!["tenant".to_string(), "participant_id".to_string()],
				referenced_table: "memory_participants".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_task_participants".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_assignment_participant".to_string(),
				columns: vec!["tenant".to_string(), "participant_id".to_string()],
				referenced_table: "memory_participants".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_unit_retention".to_string(),
			constraint: Constraint::ForeignKey {
				name: "retention_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_units".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_unit_bank".to_string(),
				columns: vec!["tenant".to_string(), "bank_id".to_string()],
				referenced_table: "memory_banks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_collections".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_collections_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "semantic_indexes".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_entries_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				referenced_table: "semantic_indexes".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_points".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_points_entry_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "entry_id".to_string()],
				referenced_table: "semantic_entries".to_string(),
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
