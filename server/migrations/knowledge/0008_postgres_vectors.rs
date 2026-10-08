// reinhardt-migration-source: 1
// Extension installation is unsupported by the typed migration operations.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0008_postgres_vectors", "knowledge")
		.add_dependency("knowledge", "0007_model_state")
		.add_operation(Operation::RunSQL {
			sql: "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pgroonga;"
				.into(),
			reverse_sql: None,
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_vector_collections".to_owned(),
			columns: vec![
				ColumnDefinition::new("collection", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("dimensions", FieldType::Integer).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "semantic_vector_dimensions".into(),
				expression: "dimensions BETWEEN 1 AND 8192".into(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_vectors".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("collection", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("embedding", FieldType::Custom("vector".into()))
					.with_not_null(true),
				ColumnDefinition::new("payload", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![Constraint::ForeignKey {
				name: "semantic_vectors_collection_fk".into(),
				columns: vec!["collection".into()],
				referenced_table: "semantic_vector_collections".into(),
				referenced_columns: vec!["collection".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.atomic(true)
}
