// reinhardt-migration-source: 1
// Logical model state; 0072 owns the physical change.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_egress_request_via_state", "execution")
		.add_dependency("execution", "0072_egress_request_via")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "egress_requests".to_string(),
			column: ColumnDefinition::new("via", FieldType::Text)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
}
