// reinhardt-migration-source: 1
// OAuth Clients, Tenant scope ceilings and personal Tenant Secrets (#554).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0043_oauth_clients_and_personal_secrets", "identity")
		.add_dependency("identity", "0042_oauth_connections_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tenant_secrets".into(),
			column: ColumnDefinition::new("owner", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "oauth_clients".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("client_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("secret_resource", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("pinned_version", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![Constraint::ForeignKey {name: "oauth_clients_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None},
				Constraint::Check {name: "oauth_client_secret_pin".into(), expression: "(pinned_version IS NULL OR secret_resource IS NOT NULL) AND revision >= 0 AND (owner IS NULL OR length(owner) > 0)".into()},
			],
			without_rowid: None, interleave_in_parent: None, partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "connection_scope_ceilings".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scopes", FieldType::Array(Box::new(FieldType::Text))).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey {name: "connection_scope_ceilings_pkey".into(), columns: vec!["tenant".into(), "resource".into()]},
Constraint::ForeignKey {name: "connection_scope_ceilings_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None},
			],
			without_rowid: None, interleave_in_parent: None, partition: None,
		})
		// Reinhardt cannot express coalesce expression indexes in migration
		// source. Keep the two expression indexes as reversible DDL assets.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0043_oauth_client_indexes.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0043_oauth_client_indexes.sql").to_owned()),
		})
		// Reinhardt has no policy operations (reinhardt-web#6770). These
		// policy assets follow database::policy, as identity/0037 does.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0043_oauth_client_row_level_security.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0043_oauth_client_row_level_security.sql").to_owned()),
		})
}
