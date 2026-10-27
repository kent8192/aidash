// reinhardt-migration-source: 1
// Logical model state; 0063 owns the physical tables, keys and indexes.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0066_previews_state", "execution")
		.add_dependency("execution", "0065_previews_row_level_security")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "core_previews".into(),
			columns: vec![
				ColumnDefinition::new("area_id", FieldType::Uuid),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("file_id", FieldType::Uuid),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("label", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("live_preview_id", FieldType::Uuid),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_preview_sessions".into(),
			columns: vec![
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("last_seen_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("origin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("preview_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("served_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_preview_tickets".into(),
			columns: vec![
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("origin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("preview_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("used_at", FieldType::TimestampTz),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
