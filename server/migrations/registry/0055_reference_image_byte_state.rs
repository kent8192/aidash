// reinhardt-migration-source: 1
// Mirror the child table and index column in the ORM model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0055_reference_image_byte_state", "registry")
		.add_dependency("registry", "0054_reference_images")
		.add_operation(Operation::AddColumn {
			table: "agent_knowledge".into(),
			column: ColumnDefinition::new("image_index", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_knowledge_images".into(),
			columns: vec![
				ColumnDefinition::new("agent_id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("agent_version", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("bytes", FieldType::Binary).with_not_null(true),
				ColumnDefinition::new("document_index", FieldType::Integer)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sha256", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.state_only(true)
		.atomic(true)
}
