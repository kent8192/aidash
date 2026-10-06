// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "knowledge")
		.database_only(true)
		.add_dependency("identity", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_agent_memory".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_agent_memory_pkey".to_owned(),
				columns: vec!["entry_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_collections".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_collections_pkey".to_owned(),
				columns: vec!["collection".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_entries_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".to_owned(),
			constraint: Constraint::Unique {
				name: "semantic_entries_workspace_id_key_key".to_owned(),
				columns: vec!["workspace_id".to_owned(), "key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_history_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_indexes".to_owned(),
			constraint: Constraint::Unique {
				name: "semantic_indexes_collection_key".to_owned(),
				columns: vec!["collection".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_indexes".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_indexes_pkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_points".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_points_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_attempts".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_remote_attempts_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_operations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_remote_operations_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_remote_reads_pkey".to_owned(),
				columns: vec![
					"grant_id".to_owned(),
					"admission_id".to_owned(),
					"entry_id".to_owned(),
					"revision".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_receipts".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_remote_receipts_pkey".to_owned(),
				columns: vec!["operation_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_run_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "semantic_run_reads_pkey".to_owned(),
				columns: vec![
					"run_id".to_owned(),
					"entry_id".to_owned(),
					"revision".to_owned(),
				],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "semantic_remote_operations".to_owned(),
			name: "semantic_remote_operation_run".to_owned(),
			columns: vec![
				"home_node".to_owned(),
				"grant_id".to_owned(),
				"admission_id".to_owned(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "semantic_remote_receipts".to_owned(),
			name: "semantic_remote_receipts_run".to_owned(),
			columns: vec!["run_id".to_owned()],
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
