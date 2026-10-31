// reinhardt-migration-source: 1
// Image rows share their parent knowledge row's Tenant and lifecycle.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0054_reference_images", "registry")
		.database_only(true)
		.add_dependency("registry", "0053_embedding_inputs")
		.add_operation(Operation::AddColumn {
			table: "agent_knowledge".into(),
			column: ColumnDefinition::new("image_index", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_knowledge_images".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document_index", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("sha256", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("bytes", FieldType::Bytea).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey { name: "agent_knowledge_images_pkey".into(), columns: vec!["tenant".into(), "agent_id".into(), "agent_version".into(), "document_index".into()] },
				Constraint::ForeignKey { name: "agent_knowledge_images_parent_fkey".into(), columns: vec!["tenant".into(), "agent_id".into(), "agent_version".into()], referenced_table: "agent_knowledge".into(), referenced_columns: vec!["tenant".into(), "agent_id".into(), "agent_version".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Check { name: "agent_knowledge_images_metadata".into(), expression: "document_index >= 0 AND media_type LIKE 'image/%' AND size = octet_length(bytes) AND sha256 ~ '^[0-9a-f]{64}$'".into() },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// PostgreSQL function bodies and row policies cannot be represented by SchemaExpr.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0054_reference_images.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0054_reference_images.sql").into()),
		})
		.atomic(true)
}
