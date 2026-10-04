// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "marketplace")
		.database_only(true)
		.add_dependency("knowledge", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_audiences".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_audiences_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_consents".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_consents_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_gate".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_gate_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_installations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_installations_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_installations".to_owned(),
			constraint: Constraint::Unique {
				name: "marketplace_installations_tenant_package_key_key".to_owned(),
				columns: vec!["tenant".to_owned(), "package_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_provenance".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_provenance_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_requests_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_revisions".to_owned(),
			constraint: Constraint::Unique {
				name: "marketplace_revisions_entry_id_entry_version_key".to_owned(),
				columns: vec!["entry_id".to_owned(), "entry_version".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_revisions".to_owned(),
			constraint: Constraint::Unique {
				name: "marketplace_revisions_installation_revision_key".to_owned(),
				columns: vec!["installation".to_owned(), "revision".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_revisions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_revisions_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_versions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "marketplace_versions_pkey".to_owned(),
				columns: vec!["key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_versions".to_owned(),
			constraint: Constraint::Unique {
				name: "marketplace_versions_repository_owner_package_id_version_key".to_owned(),
				columns: vec![
					"repository".to_owned(),
					"owner".to_owned(),
					"package_id".to_owned(),
					"version".to_owned(),
				],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "marketplace_audiences".to_owned(),
			name: "marketplace_audiences_tenants".to_owned(),
			columns: vec![],
			unique: false,
			index_type: Some(IndexType::Gin),
			where_clause: None,
			concurrently: false,
			expressions: Some(vec![r#"((document -> 'tenants'::text))"#.to_owned()]),
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "marketplace_versions".to_owned(),
			name: "marketplace_versions_source_content".to_owned(),
			columns: vec![
				"owner".to_owned(),
				"source_id".to_owned(),
				"source_version".to_owned(),
				"source_content".to_owned(),
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
