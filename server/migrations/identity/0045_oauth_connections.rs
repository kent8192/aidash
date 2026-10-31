// reinhardt-migration-source: 1
// MCP OAuth Connections (ADR 0100). Policy DDL is the documented Reinhardt
// exception (#6770); SQL assets follow database::policy.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_oauth_connections", "identity")
		.add_dependency("identity", "0044_catalog_approval_requests_state")
		.atomic(true)
		.database_only(true)
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
			constraints: vec![Constraint::ForeignKey {
 name: "oauth_connections_tenant_fkey".into(), columns:vec!["tenant".into()],
 referenced_table:"tenants".into(),referenced_columns:vec!["name".into()],
 on_delete:ForeignKeyAction::NoAction,on_update:ForeignKeyAction::NoAction,deferrable:None,
 },Constraint::Check{name:"oauth_connections_state".into(),expression:"state IN ('pending','active','needs_reconnection') AND revision > 0 AND (state <> 'active' OR pinned_version <> '')".into()}],
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
			constraints: vec![Constraint::ForeignKey {
 name: "oauth_authorizations_tenant_fkey".into(), columns:vec!["tenant".into()],
 referenced_table:"tenants".into(),referenced_columns:vec!["name".into()],
 on_delete:ForeignKeyAction::NoAction,on_update:ForeignKeyAction::NoAction,deferrable:None,
 }],
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
			constraints: vec![Constraint::ForeignKey {
 name: "oauth_dynamic_clients_tenant_fkey".into(), columns:vec!["tenant".into()],
 referenced_table:"tenants".into(),referenced_columns:vec!["name".into()],
 on_delete:ForeignKeyAction::NoAction,on_update:ForeignKeyAction::NoAction,deferrable:None,
 }],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "oauth_connections".into(),
			name: "oauth_connections_owner_resource".into(),
			columns: vec!["tenant".into(), "subject".into(), "resource".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "oauth_dynamic_clients".into(),
			name: "oauth_dynamic_clients_key".into(),
			columns: vec!["tenant".into(), "issuer".into(), "redirect_uri".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0045_oauth_connections.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0045_oauth_connections.sql").to_owned()),
		})
}
