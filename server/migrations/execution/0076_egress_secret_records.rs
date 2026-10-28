// reinhardt-migration-source: 1
// The Tenant Secret a brokered Egress request used, never its value (ADR 0095).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0076_egress_secret_records", "execution")
		.add_dependency("execution", "0075_invocation_tenant_secrets")
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("secret_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("secret_version", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("secret_host", FieldType::Text),
			mysql_options: None,
		})
		.atomic(true)
}
