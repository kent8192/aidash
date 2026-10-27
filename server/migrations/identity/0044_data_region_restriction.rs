// reinhardt-migration-source: 1
// Existing Tenants and historical policies remain unrestricted by default.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0044_data_region_restriction", "identity")
		.add_dependency("identity", "0043_tenant_data_protection_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection".into(),
			column: ColumnDefinition::new("data_region_restricted", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection_history".into(),
			column: ColumnDefinition::new("data_region_restricted", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
}
