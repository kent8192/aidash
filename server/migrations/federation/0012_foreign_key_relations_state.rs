// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0012_foreign_key_relations_state", "federation")
		.add_dependency("federation", "0011_tenant_owned_row_level_security")
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_gate".to_string(),
			constraint: Constraint::ForeignKey {
				name: "atomic_gate_transaction_id_fkey".to_string(),
				columns: vec!["transaction_id".to_string()],
				referenced_table: "atomic_participants".to_string(),
				referenced_columns: vec!["id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mapping_history".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mapping_history_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mappings".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mappings_credential_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "credential_id".to_string()],
				referenced_table: "authorization_credentials".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_grants".to_string(),
			constraint: Constraint::ForeignKey {
				name: "authorization_remote_grants_task_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "task_id".to_string()],
				referenced_table: "tasks".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
