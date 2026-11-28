// reinhardt-migration-source: 1
//! The ORM schema. Drive query indexes, Tenant fences and foreign keys are
//! installed by the database-only migrations.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0063_drive_index_state", "knowledge")
		.add_dependency("knowledge", "0062_drive_index")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "drive_index_requests".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("version", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("requester", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("estimated_micros", FieldType::BigInteger)
					.with_not_null(false),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_passages".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("embedding_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("ordinal", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("start_line", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("end_line", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("body", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("vector", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
