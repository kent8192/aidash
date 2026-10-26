// reinhardt-migration-source: 1
// The Tenant Secret a brokered Egress request used, never its value (ADR 0095).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0061_egress_secret_records", "execution")
		// W3b contributes execution/0060 for tool invocation records; merge this
		// history after it and make 0061 depend on that head rather than fork
		// the Execution graph.
		.add_dependency("execution", "0059_research_history_merge")
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
