// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "marketplace")
		.database_only(true)
		.add_dependency("knowledge", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_audiences".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_consents".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_gate".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_installations".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("package_key", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_provenance".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_requests".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_revisions".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("installation", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_version", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "marketplace_versions".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("repository", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("package_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_content", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
