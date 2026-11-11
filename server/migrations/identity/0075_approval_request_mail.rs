// reinhardt-migration-source: 1
// Mail for Catalog Approval Requests is on for existing people (ADR 0128).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0075_approval_request_mail", "identity")
		.add_dependency("identity", "0074_approval_carry_over_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "dashboard_identities".into(),
			column: ColumnDefinition::new("approval_request_mail", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("true".into())),
			mysql_options: None,
		})
}
