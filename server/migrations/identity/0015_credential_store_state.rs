// reinhardt-migration-source: 1
// Binary renders as BINARY at the pinned Reinhardt revision (#6637).
// Physical bytea DDL precedes the equivalent Binary model snapshot.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0015_credential_store_state", "identity")
		.add_dependency("identity", "0014_credential_store_schema")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "credential_store_resources".into(),
			columns: vec![
				ColumnDefinition::new("resource", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("next_version", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "credential_store_versions".into(),
			columns: vec![
				ColumnDefinition::new("resource", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("version", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("key_id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("algorithm", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("nonce", FieldType::Binary)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("ciphertext", FieldType::Binary)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("state", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("disabled_at", FieldType::TimestampTz)
					.with_not_null(false)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "credential_store_keys".into(),
			columns: vec![
				ColumnDefinition::new("key_id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("check_nonce", FieldType::Binary)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("check_ciphertext", FieldType::Binary)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
