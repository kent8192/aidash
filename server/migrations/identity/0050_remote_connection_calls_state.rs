// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_remote_connection_calls_state", "identity")
		.add_dependency("identity", "0049_remote_connection_calls")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "remote_connection_calls".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("admission_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("idempotency_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tool_key", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("arguments_hash", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("connection_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("secret_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("secret_version", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("secret_host", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
