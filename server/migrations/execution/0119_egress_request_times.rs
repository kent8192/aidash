// reinhardt-migration-source: 1
// What a request cost at the gateway, outside `duration_ms` (#752).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0119_egress_request_times", "execution")
		.add_dependency("execution", "0118_blob_uploads")
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("admission_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("settle_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.atomic(true)
}
