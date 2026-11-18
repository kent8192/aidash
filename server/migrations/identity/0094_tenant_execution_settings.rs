// reinhardt-migration-source: 1
// A Tenant's execution settings (#1009): its Lease Recovery limit, 1 to 20,
// with a revision for optimistic updates. A Tenant with no row uses 5.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0094_tenant_execution_settings", "identity")
		.add_dependency("identity", "0093_standing_instructions_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_execution_settings".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("lease_recovery_limit", FieldType::Integer)
					.with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "tenant_execution_settings_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenant_execution_settings_valid".into(),
					expression: "revision > 0 AND lease_recovery_limit BETWEEN 1 AND 20".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
