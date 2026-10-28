// reinhardt-migration-source: 1
// Logical model state; 0070 owns the physical change.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0071_egress_session_requests_state", "execution")
		.add_dependency("execution", "0070_egress_session_requests")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AlterColumn {
			table: "egress_requests".to_string(),
			column: "operation_id".to_string(),
			old_definition: Some(
				ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("operation_id", FieldType::Uuid)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
}
