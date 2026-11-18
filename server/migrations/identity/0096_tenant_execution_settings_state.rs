// reinhardt-migration-source: 1
// ORM state of Tenant execution settings (#1009); physical keys, checks and RLS
// are database-only in 0094 and 0095.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0096_tenant_execution_settings_state", "identity")
		.add_dependency(
			"identity",
			"0095_tenant_execution_settings_row_level_security",
		)
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_execution_settings".into(),
			columns: vec![
				ColumnDefinition::new("lease_recovery_limit", FieldType::Integer)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_by", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
