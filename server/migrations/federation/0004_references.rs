// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "federation")
		.database_only(true)
		.add_dependency("execution", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_gate".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "atomic_gate_transaction_id_fkey".to_owned(),
				columns: vec!["transaction_id".to_owned()],
				referenced_table: "atomic_participants".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_peer_trust".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "atomic_peer_trust_node_id_fkey".to_owned(),
				columns: vec!["node_id".to_owned()],
				referenced_table: "peers".to_owned(),
				referenced_columns: vec!["node_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_votes".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "atomic_votes_transaction_id_fkey".to_owned(),
				columns: vec!["transaction_id".to_owned()],
				referenced_table: "atomic_coordinators".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mapping_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mapping_history_credential_id_fkey".to_owned(),
				columns: vec!["credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mapping_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mapping_history_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mappings".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mappings_credential_id_fkey".to_owned(),
				columns: vec!["credential_id".to_owned()],
				referenced_table: "authorization_credentials".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mappings".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_peer_mappings_tenant_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "authorization_bundles".to_owned(),
				referenced_columns: vec!["tenant".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_grant_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_remote_grant_reads_grant_id_fkey".to_owned(),
				columns: vec!["grant_id".to_owned()],
				referenced_table: "authorization_remote_grants".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_grants".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "authorization_remote_grants_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "delegations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "delegations_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "remote_run_message_fences".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "remote_run_message_fences_task_id_fkey".to_owned(),
				columns: vec!["task_id".to_owned()],
				referenced_table: "tasks".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
