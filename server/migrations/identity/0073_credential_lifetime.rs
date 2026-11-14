// reinhardt-migration-source: 1
// Tenant lifetime ceilings and the reason an expiry was shortened.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_credential_lifetime", "identity")
		.add_dependency("identity", "0072_remote_execution_activation")
		.add_operation(Operation::AddColumn {
			table: "tenants".into(),
			column: ColumnDefinition::new("credential_max_seconds", FieldType::BigInteger)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_credentials".into(),
			column: ColumnDefinition::new("end_reason", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
