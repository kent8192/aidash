// reinhardt-migration-source: 1
// Demo data (ADR 0079, ADR 0080). `demo_data_loads` records each pair of inputs
// the `demo-data` command finished loading, `demo_tenants` the Tenant each
// structure key was assigned, and `demo_reservations` the Demo Reservations
// sign-in spends. Typed operations express the whole schema; the Tenant policy
// of `demo_reservations` follows in 0035.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0033_demo_data", "identity")
		.database_only(true)
		.add_dependency("identity", "0032_tenant_owned_row_level_security")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "demo_data_loads".into(),
			columns: vec![
				ColumnDefinition::new("digest", FieldType::Text).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("loaded_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
			],
			constraints: vec![Constraint::Check {
				name: "demo_data_loads_digest".into(),
				expression: "digest ~ '^[0-9a-f]{64}$'".into(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "demo_tenants".into(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				// One structure key per Tenant: a rerun finds the Tenant by its key.
				Constraint::Unique { name: "demo_tenants_tenant_key".into(), columns: vec!["tenant".into()] },
				Constraint::ForeignKey {
					name: "demo_tenants_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "demo_tenants_key".into(),
					expression: "key ~ '^[a-z][a-z0-9-]{0,31}$'".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "demo_reservations".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("email", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
				ColumnDefinition::new("spent_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("spent_by", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::Unique {
					name: "demo_reservations_tenant_email_key".into(),
					columns: vec!["tenant".into(), "email".into()],
				},
				Constraint::ForeignKey {
					name: "demo_reservations_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "demo_reservations_spent_by_fkey".into(),
					columns: vec!["spent_by".into()],
					referenced_table: "dashboard_identities".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				// An address is compared lowercased, and a spent reservation says when
				// and on whom.
				Constraint::Check {
					name: "demo_reservations_state".into(),
					expression: "email = lower(email) AND email <> '' AND subject <> '' AND (spent_at IS NULL) = (spent_by IS NULL)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
