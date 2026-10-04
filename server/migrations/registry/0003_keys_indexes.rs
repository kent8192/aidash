// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "registry")
		.database_only(true)
		.add_dependency("marketplace", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_draft_registrations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_draft_registrations_pkey".to_owned(),
				columns: vec!["draft_id".to_owned(), "revision".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_draft_shares".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_draft_shares_pkey".to_owned(),
				columns: vec!["draft_id".to_owned(), "subject".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_drafts".to_owned(),
			constraint: Constraint::Unique {
				name: "agent_drafts_managed_id_key".to_owned(),
				columns: vec!["managed_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_drafts".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_drafts_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_incident_events".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_incident_events_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_incidents".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_incidents_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_knowledge".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_knowledge_pkey".to_owned(),
				columns: vec!["agent_id".to_owned(), "agent_version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_limits".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_test_limits_pkey".to_owned(),
				columns: vec!["tenant".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_profiles".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_test_profiles_pkey".to_owned(),
				columns: vec!["tenant".to_owned(), "id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_sessions".to_owned(),
			constraint: Constraint::Unique {
				name: "agent_test_sessions_active_slot_key".to_owned(),
				columns: vec!["active_slot".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_sessions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "agent_test_sessions_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "installations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "installations_pkey".to_owned(),
				columns: vec!["id".to_owned(), "version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "packages".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "packages_pkey".to_owned(),
				columns: vec!["id".to_owned(), "version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_model_refs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "registry_agent_model_refs_pkey".to_owned(),
				columns: vec!["agent_id".to_owned(), "agent_version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_resource_refs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "registry_agent_resource_refs_pkey".to_owned(),
				columns: vec![
					"agent_id".to_owned(),
					"agent_version".to_owned(),
					"required_kind".to_owned(),
					"ordinal".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "registry_pkey".to_owned(),
				columns: vec!["id".to_owned(), "version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "registry_requests_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "agent_draft_registrations".to_owned(),
			name: "agent_draft_registered_version".to_owned(),
			columns: vec!["agent_id".to_owned(), "version".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "agent_drafts".to_owned(),
			name: "agent_drafts_tenant_owner".to_owned(),
			columns: vec!["tenant".to_owned(), "owner".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "agent_incidents".to_owned(),
			name: "agent_incidents_version".to_owned(),
			columns: vec!["agent_id".to_owned(), "version".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "agent_test_sessions".to_owned(),
			name: "agent_test_sessions_draft_created".to_owned(),
			columns: vec!["draft_id".to_owned(), "created_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "registry".to_owned(),
			name: "registry_id_version_kind_unique".to_owned(),
			columns: vec!["id".to_owned(), "version".to_owned(), "kind".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "registry".to_owned(),
			name: "registry_metadata".to_owned(),
			columns: vec!["metadata".to_owned()],
			unique: false,
			index_type: Some(IndexType::Gin),
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
