// reinhardt-migration-source: 1
// Physical schema for the Tenant root record (ADR 0050), Personal Tenant links,
// Invitations and addressed Registration Requests. Bytea and the constraints
// are physical; 0021 records the matching model state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0020_tenancy_schema", "identity")
		.add_dependency("identity", "0019_provider_authorizations")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "tenants".into(),
			columns: vec![
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("label", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("personal", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".into())),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_by_identity", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "tenants_created_by_identity_fkey".into(),
					columns: vec!["created_by_identity".into()],
					referenced_table: "dashboard_identities".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenants_root".into(),
					expression: "length(name) BETWEEN 1 AND 256 AND created_by_kind IN ('identity','deployment_administrator') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND (label IS NULL OR length(label) BETWEEN 1 AND 256)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "tenants".into(),
			name: "tenants_creator".into(),
			columns: vec!["created_by_identity".into(), "created_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("created_by_identity IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "personal_tenants".into(),
			columns: vec![
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::Unique { name: "personal_tenants_tenant_key".into(), columns: vec!["tenant".into()] },
				Constraint::ForeignKey {
					name: "personal_tenants_identity_id_fkey".into(),
					columns: vec!["identity_id".into()],
					referenced_table: "dashboard_identities".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "personal_tenants_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "tenant_invitations".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("token_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("groups", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'[]'::jsonb".into())),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("issued_by_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("issued_by_identity", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("revoked_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("redeemed_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("redeemed_by", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("registration_id", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::Unique { name: "tenant_invitations_token_hash_key".into(), columns: vec!["token_hash".into()] },
				Constraint::ForeignKey {
					name: "tenant_invitations_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenant_invitations_state".into(),
					expression: "octet_length(token_hash) = 32 AND jsonb_typeof(groups) = 'array' AND expires_at > created_at AND issued_by_kind IN ('deployment_administrator','tenant_administrator') AND (redeemed_at IS NULL) = (redeemed_by IS NULL)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "tenant_invitations".into(),
			name: "tenant_invitations_tenant".into(),
			columns: vec!["tenant".into(), "expires_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_registration_requests".into(),
			column: ColumnDefinition::new("tenant", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_registration_requests".into(),
			column: ColumnDefinition::new("invitation_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_administration_history".into(),
			column: ColumnDefinition::new("invitation_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		// Tenant creation by the Deployment Administrator bearer token and
		// Invitation issuance concern no External Identity. The pinned typed
		// AlterColumn renders no NOT NULL change for PostgreSQL, so this one
		// statement is a documented RunSQL exception (kent8192/reinhardt-web#6803).
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0020_history_identity_nullable.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0020_history_identity_nullable.sql").to_owned(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0020_tenancy_backfill.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0020_tenancy_backfill.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".into(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_registration_requests_tenant_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_registration_requests".into(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_registration_requests_invitation_id_fkey".into(),
				columns: vec!["invitation_id".into()],
				referenced_table: "tenant_invitations".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_bundles".into(),
			constraint: Constraint::ForeignKey {
				name: "authorization_bundles_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
