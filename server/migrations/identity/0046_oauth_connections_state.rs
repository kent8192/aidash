// reinhardt-migration-source: 1
// ORM state for OAuth Connection tables. Physical references, checks, indexes
// and policies stay database-only in 0045, following the 0036/0038 precedent.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_oauth_connections_state", "identity")
		.add_dependency("identity", "0045_oauth_connections")
		.atomic(true)
		.state_only(true)
		.database_only(false)
		.add_operation(Operation::CreateTable {
			name: "oauth_connections".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("client", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("scopes", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("secret_resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("pinned_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("access_expires_at", FieldType::TimestampTz)
					.with_not_null(false),
				ColumnDefinition::new("refresh_expires_at", FieldType::TimestampTz)
					.with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "oauth_authorizations".into(),
			columns: vec![
				ColumnDefinition::new("state_hash", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("browser_hash", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("session_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("context", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("client", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("scopes", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("pkce_verifier", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("redirect_uri", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("return_to", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("connection", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "oauth_dynamic_clients".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("redirect_uri", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("client_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("secret_resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("pinned_version", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
