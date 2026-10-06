// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "identity")
		.database_only(true)
		.add_dependency("federation", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_bundles".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_bundles_pkey".to_owned(),
				columns: vec!["tenant".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_catalog_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_catalog_history_pkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
					"revision".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_catalog".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_catalog_pkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_credentials".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_credentials_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_credentials".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_credentials_token_hash_key".to_owned(),
				columns: vec!["token_hash".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_decisions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_decisions_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_execution_pkey".to_owned(),
				columns: vec!["run_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_execution".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_execution_task_id_key".to_owned(),
				columns: vec!["task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_graph_operator_grants".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_graph_operator_grants_pkey".to_owned(),
				columns: vec![
					"source_node".to_owned(),
					"source_operator".to_owned(),
					"tenant".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_commands".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_commands_pkey".to_owned(),
				columns: vec!["grant_id".to_owned(), "request_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_execution".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_remote_execution_admission_id_key".to_owned(),
				columns: vec!["admission_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_execution".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_execution_pkey".to_owned(),
				columns: vec!["grant_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_execution".to_owned(),
			constraint: Constraint::Unique {
				name: "authorization_remote_execution_task_id_key".to_owned(),
				columns: vec!["task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_remote_outputs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_remote_outputs_pkey".to_owned(),
				columns: vec![
					"grant_id".to_owned(),
					"workspace_id".to_owned(),
					"resource_kind".to_owned(),
					"resource_id".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_revisions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_revisions_pkey".to_owned(),
				columns: vec!["tenant".to_owned(), "revision".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_outputs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_run_outputs_pkey".to_owned(),
				columns: vec![
					"run_id".to_owned(),
					"resource_kind".to_owned(),
					"resource_id".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_run_reads_pkey".to_owned(),
				columns: vec![
					"run_id".to_owned(),
					"resource_kind".to_owned(),
					"resource_id".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_registry_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_run_registry_reads_pkey".to_owned(),
				columns: vec![
					"run_id".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_remote_reads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_run_remote_reads_pkey".to_owned(),
				columns: vec![
					"run_id".to_owned(),
					"node_id".to_owned(),
					"entry_id".to_owned(),
					"entry_version".to_owned(),
					"digest".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_task_origins".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_task_origins_pkey".to_owned(),
				columns: vec!["task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_workspaces".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "authorization_workspaces_pkey".to_owned(),
				columns: vec!["workspace_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_execution_origins".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_execution_origins_pkey".to_owned(),
				columns: vec!["run_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_identities".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_identities_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_identities".to_owned(),
			constraint: Constraint::Unique {
				name: "dashboard_identity_key".to_owned(),
				columns: vec!["issuer".to_owned(), "subject".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_login_transactions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_login_transactions_pkey".to_owned(),
				columns: vec!["state_hash".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_logout_tokens".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_logout_tokens_pkey".to_owned(),
				columns: vec!["jti_hash".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_owned(),
			constraint: Constraint::Unique {
				name: "dashboard_mapping_key".to_owned(),
				columns: vec![
					"identity_id".to_owned(),
					"tenant".to_owned(),
					"subject".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_owned(),
			constraint: Constraint::Unique {
				name: "dashboard_mappings_credential_id_key".to_owned(),
				columns: vec!["credential_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_mappings".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_mappings_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_operator_grants".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_operator_grants_pkey".to_owned(),
				columns: vec!["identity_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_registration_requests_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_sessions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "dashboard_sessions_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_sessions".to_owned(),
			constraint: Constraint::Unique {
				name: "dashboard_sessions_token_hash_key".to_owned(),
				columns: vec!["token_hash".to_owned()],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "authorization_credentials".to_owned(),
			name: "authorization_credentials_tenant".to_owned(),
			columns: vec![
				"tenant".to_owned(),
				"created_at".to_owned(),
				"id".to_owned(),
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
			table: "authorization_decisions".to_owned(),
			name: "authorization_decisions_tenant".to_owned(),
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
			table: "authorization_run_outputs".to_owned(),
			name: "authorization_output_resource".to_owned(),
			columns: vec![
				"workspace_id".to_owned(),
				"resource_kind".to_owned(),
				"resource_id".to_owned(),
				"run_id".to_owned(),
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
			table: "authorization_workspaces".to_owned(),
			name: "authorization_workspaces_tenant".to_owned(),
			columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_login_transactions".to_owned(),
			name: "dashboard_login_browser".to_owned(),
			columns: vec!["browser_hash".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_login_transactions".to_owned(),
			name: "dashboard_login_expiry".to_owned(),
			columns: vec!["expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_logout_tokens".to_owned(),
			name: "dashboard_logout_expiry".to_owned(),
			columns: vec!["expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_execution_origins".to_owned(),
			name: "dashboard_origin_identity_run".to_owned(),
			columns: vec!["identity_id".to_owned(), "run_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_registration_requests".to_owned(),
			name: "dashboard_registration_pending".to_owned(),
			columns: vec!["status".to_owned(), "expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_sessions".to_owned(),
			name: "dashboard_session_expiry".to_owned(),
			columns: vec!["expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_sessions".to_owned(),
			name: "dashboard_session_identity".to_owned(),
			columns: vec!["identity_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_sessions".to_owned(),
			name: "dashboard_session_provider_sid".to_owned(),
			columns: vec!["provider_sid".to_owned()],
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
