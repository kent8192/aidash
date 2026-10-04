// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "federation")
		.database_only(true)
		.add_dependency("execution", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_authority_attempts".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_authority_attempts_pkey".to_owned(),
				columns: vec!["transaction_id".to_owned(), "node_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_coordinators".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_coordinators_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_gate".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_gate_pkey".to_owned(),
				columns: vec!["singleton".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_history_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_participants".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_participants_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_peer_trust".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_peer_trust_pkey".to_owned(),
				columns: vec!["node_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_preflights".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_preflights_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_subjects".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_subjects_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "atomic_votes".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "atomic_votes_pkey".to_owned(),
				columns: vec!["transaction_id".to_owned(), "node_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mapping_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_peer_mapping_history_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_mappings".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_peer_mappings_pkey".to_owned(),
				columns: vec![
					"source_node".to_owned(),
					"source_tenant".to_owned(),
					"source_subject".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_admissions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_admissions_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_admissions".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_remote_admissions_source_node_grant_id_key".to_owned(),
				columns: vec!["source_node".to_owned(), "grant_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_admissions".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_remote_admissions_source_node_task_id_key".to_owned(),
				columns: vec!["source_node".to_owned(), "task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_grant_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_grant_reads_pkey".to_owned(),
				columns: vec![
					"grant_id".to_owned(),
					"workspace_id".to_owned(),
					"resource_kind".to_owned(),
					"resource_id".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_grants".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_grants_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "delegations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "delegations_pkey".to_owned(),
				columns: vec!["task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "peer_events".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "peer_events_pkey".to_owned(),
				columns: vec!["node_id".to_owned(), "event_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "peers".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "peers_pkey".to_owned(),
				columns: vec!["node_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "remote_run_message_fences".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "remote_run_message_fences_pkey".to_owned(),
				columns: vec!["task_id".to_owned(), "idempotency_key".to_owned()],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "atomic_authority_attempts".to_owned(),
			name: "atomic_authority_pending".to_owned(),
			columns: vec!["transaction_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(outcome IS NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "atomic_authority_attempts".to_owned(),
			name: "atomic_authority_pending_peer".to_owned(),
			columns: vec!["node_id".to_owned(), "transaction_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(outcome IS NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "authorization_peer_mapping_history".to_owned(),
			name: "authorization_peer_mapping_history_tenant_sequence".to_owned(),
			columns: vec!["tenant".to_owned(), "sequence".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "delegations".to_owned(),
			name: "delegations_retry".to_owned(),
			columns: vec!["next_attempt_at".to_owned(), "created_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(NOT delivered)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "remote_run_message_fences".to_owned(),
			name: "remote_run_message_fences_sequence".to_owned(),
			columns: vec![
				"task_id".to_owned(),
				"run_id".to_owned(),
				"input_seq".to_owned(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
