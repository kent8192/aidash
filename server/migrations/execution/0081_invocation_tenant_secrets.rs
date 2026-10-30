// reinhardt-migration-source: 1
//! Record the Tenant Secret a tool invocation carried (ADR 0095). The columns
//! are null for calls without one and never hold the value.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0081_invocation_tenant_secrets", "execution")
		.add_dependency("execution", "0080_invocation_delivery")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("tenant_secret_host", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("tenant_secret_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("tenant_secret_version", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
}
