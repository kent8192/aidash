// reinhardt-migration-source: 1
// Tenant Secrets (ADR 0051): the metadata of each named, write-only value a
// Tenant stores for its tools. The value itself lives in the Secret Store
// under `secret_resource`. A name is unique among a Tenant's Tenant Secrets
// that are not deleted, so a deleted one frees its name while its Secret
// Store contents are still being cleaned up.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0036_tenant_secrets", "identity")
		.database_only(true)
		.add_dependency("identity", "0035_demo_reservations_row_level_security")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_secrets".into(),
			columns: vec![
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("fingerprint", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("hosts", FieldType::Array(Box::new(FieldType::Text))).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("last4", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("pinned_version", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("secret_resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "tenant_secrets_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenant_secrets_state".into(),
					expression: "state IN ('pending','active','deleted') AND (state <> 'active' OR pinned_version IS NOT NULL) AND (state <> 'pending' OR pinned_version IS NULL) AND revision > 0".into(),
				},
				Constraint::Check {
					name: "tenant_secrets_metadata".into(),
					expression: "name ~ '^[a-z0-9][a-z0-9_-]{0,63}$' AND cardinality(hosts) BETWEEN 1 AND 16 AND id::text ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$' AND fingerprint ~ '^[0-9a-f]{16}$' AND length(last4) BETWEEN 1 AND 4 AND secret_resource = 'aidash-store/tenant-secrets/' || id::text".into(),
				},
				Constraint::Check {
					name: "tenant_secrets_pin".into(),
					expression: "pinned_version IS NULL OR (pinned_version ~ '/versions/[1-9][0-9]*$' AND left(pinned_version,length(secret_resource)+10) = secret_resource || '/versions/')".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// A Tenant's name is unique among the Tenant Secrets that are not deleted.
		.add_operation(Operation::CreateNamedIndex {
			table: "tenant_secrets".into(),
			name: "tenant_secrets_live_name".into(),
			columns: vec!["tenant".into(), "name".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("state <> 'deleted'".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
